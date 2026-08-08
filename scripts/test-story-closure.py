#!/usr/bin/env python3
"""Tests for `check-story-closure.py`, against boards whose right answer is written down here.

The whole product of that checker is **one distinction**: a story that was implemented and never
closed, told apart from a story that is being implemented right now. Both carry ticked Acceptance
rows, and a check that cannot separate them is a check people learn to scroll past — which is worse
than no check, because it also occupies the slot a working one would have.

So the fixtures are built as pairs. Every state that must be reported has a sibling that must stay
quiet, and the quiet half is asserted at least as hard as the loud half:

    reported                                  quiet
    ----------------------------------------  ------------------------------------------------
    `ready`, every row ticked                  `in-progress`, every row ticked
    `backlog`, one row outstanding (`A-16`)    `ready`, half the rows ticked (`X-29`'s partial)
    staged ticks, status not moved             the same ticks in the working tree only

Everything runs the real script in a throwaway repository, because the committed-versus-working-tree
half of the rule is git behaviour and a fixture that stubbed git would assert the stub.
"""

import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True

SCRIPT = pathlib.Path(__file__).resolve().parent / "check-story-closure.py"

#: A story whose Acceptance rows are supplied per fixture. The frontmatter carries everything the
#: board's own parser needs and nothing else, so a field added to the real template cannot silently
#: change what these tests mean.
STORY = """---
id: {id}
title: {title}
pillar: Build
status: {status}
priority: 1
epic: fixtures
---

# {title}

## Goal

A fixture story.

## Acceptance

{rows}

## Notes

Nothing here is read by the checker.
"""


class BoardCase(unittest.TestCase):
    """A throwaway repository with a board in it, and the checker run against that board."""

    def git(self, repo, *args, check=True):
        return subprocess.run(
            [
                "git",
                "-c",
                "user.name=story closure test",
                "-c",
                "user.email=story-closure@example.invalid",
                "-c",
                "commit.gpgsign=false",
                *args,
            ],
            cwd=repo,
            capture_output=True,
            text=True,
            check=check,
        )

    def commit(self, repo, message):
        self.git(repo, "add", "-A")
        self.git(repo, "commit", "-q", "--no-verify", "-m", message)

    def repo(self):
        """An initialised repository with a `scripts/` and a `docs/stories/`, and no commits yet."""
        root = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root, ignore_errors=True)
        (root / "scripts").mkdir()
        (root / "docs" / "stories").mkdir(parents=True)
        shutil.copy(SCRIPT, root / "scripts" / SCRIPT.name)
        self.git(root, "init", "-q")
        return root

    def write_story(self, repo, story_id, status, marks):
        """Write one fixture story. `marks` is one character per Acceptance row: `x`, ` ` or `~`."""
        rows = "\n".join(
            f"- [{mark}] Acceptance row {number}, which says something a story would say."
            for number, mark in enumerate(marks, start=1)
        )
        path = repo / "docs" / "stories" / f"{story_id}-a-fixture-story.md"
        path.write_text(
            STORY.format(id=story_id, title=f"Fixture {story_id}", status=status, rows=rows),
            encoding="utf-8",
        )
        return path

    def run_check(self, repo, *args):
        return subprocess.run(
            [sys.executable, str(repo / "scripts" / SCRIPT.name), *args],
            cwd=repo,
            capture_output=True,
            text=True,
            check=False,
        )

    def assertReports(self, result, story_id):
        self.assertIn(
            story_id,
            result.stdout,
            f"{story_id} must be named in the report:\n{result.stdout}{result.stderr}",
        )

    def assertQuietAbout(self, result, story_id):
        self.assertNotIn(
            story_id,
            result.stdout,
            f"{story_id} must not be reported — this is the benign case:\n"
            f"{result.stdout}{result.stderr}",
        )


class TheDefect(BoardCase):
    """The shape this check exists for: the work landed, and the status never followed."""

    def test_a_story_left_open_with_every_row_ticked_is_reported(self):
        repo = self.repo()
        self.write_story(repo, "A-1", "ready", "xxxx")
        self.commit(repo, "land the work, and forget to close the story")

        result = self.run_check(repo)
        self.assertReports(result, "A-1")

    def test_the_a16_shape_is_reported(self):
        """`A-16`: seven of eight rows ticked, the eighth being *the gate is green*, status `backlog`.

        This is the case the story was filed over and the reason the threshold is not *every row*.
        An implementor cannot honestly tick a gate row before the wave gate has run, so a rule that
        demanded a complete Acceptance would have been silent about the one story it was written for.
        """
        repo = self.repo()
        self.write_story(repo, "A-16", "backlog", "xxxxxxx ")
        self.commit(repo, "deliver the spec, and never run /track:done")

        result = self.run_check(repo)
        self.assertReports(result, "A-16")

    def test_the_report_names_the_counts(self):
        """Row 1 of the Acceptance: the story *and* the counts, so the reader can judge it."""
        repo = self.repo()
        self.write_story(repo, "A-16", "backlog", "xxxxxxx ")
        self.commit(repo, "deliver the spec, and never run /track:done")

        result = self.run_check(repo)
        self.assertIn("7 of 8", result.stdout, result.stdout)

    def test_both_open_statuses_are_read(self):
        """`backlog` and `ready` are both statuses a selection pass promotes from."""
        repo = self.repo()
        self.write_story(repo, "A-1", "backlog", "xxxx")
        self.write_story(repo, "A-2", "ready", "xxxx")
        self.commit(repo, "two stories nobody closed")

        result = self.run_check(repo)
        self.assertReports(result, "A-1")
        self.assertReports(result, "A-2")


class TheBenignCase(BoardCase):
    """Row 2 of the Acceptance, which is the whole difficulty.

    Ticked rows are normal. Implementors tick as they go, coordinators land partial work
    deliberately, and every one of those states has ticks in it. What separates them from the defect
    is *whether anybody is still working the story* — which the lifecycle already records, in the
    status word and in whether the ticks have been integrated at all.
    """

    def test_a_story_being_implemented_is_not_reported(self):
        """`in-progress` is the lifecycle's own word for *an implementor is ticking rows right now*.

        Reporting it would fire on every story under active work, every day. That is the check
        nobody reads.
        """
        repo = self.repo()
        self.write_story(repo, "M-70", "in-progress", "xxxxx")
        self.commit(repo, "an implementor ticking as it goes")

        self.assertQuietAbout(self.run_check(repo), "M-70")

    def test_a_partially_landed_story_is_not_reported(self):
        """`X-29`'s real shape: *land the verified half, keep the story open*, three of six ticked.

        A story with genuine work outstanding is legitimately open whatever its status says, so the
        outstanding-row threshold — not the status — is what keeps this quiet.
        """
        repo = self.repo()
        self.write_story(repo, "X-29", "ready", "xxx   ")
        self.commit(repo, "land X-29's verified half, keep the story open")

        self.assertQuietAbout(self.run_check(repo), "X-29")

    def test_a_blocked_story_is_not_reported(self):
        """`blocked` is a story parked part-way, which is exactly when it carries partial ticks."""
        repo = self.repo()
        self.write_story(repo, "B-1", "blocked", "xxxx")
        self.commit(repo, "park it on a dependency")

        self.assertQuietAbout(self.run_check(repo), "B-1")

    def test_a_closed_story_is_not_reported(self):
        repo = self.repo()
        self.write_story(repo, "D-1", "done", "xxxx")
        self.commit(repo, "close it properly")

        self.assertQuietAbout(self.run_check(repo), "D-1")

    def test_an_open_story_nobody_has_started_is_not_reported(self):
        repo = self.repo()
        self.write_story(repo, "R-1", "ready", "    ")
        self.commit(repo, "file a story")

        self.assertQuietAbout(self.run_check(repo), "R-1")

    def test_a_row_that_is_neither_ticked_nor_open_counts_as_outstanding(self):
        """`[~]` appears in this board for a row that was recast rather than satisfied.

        Any row that is not `[x]` is outstanding. Reading `~` as satisfied would report stories whose
        author explicitly wrote down that the row was *not*.
        """
        repo = self.repo()
        self.write_story(repo, "T-1", "ready", "xx~~")
        self.commit(repo, "two rows recast")

        self.assertQuietAbout(self.run_check(repo), "T-1")


class TheCommittedBoundary(BoardCase):
    """The second half of the rule: an implementation in flight has not landed anywhere yet.

    What makes the defect a defect is that the work *reached the branch selection reads from* and the
    status did not follow. Ticks that exist only in somebody's working tree are the ordinary middle of
    an implementation, and reading them would make this check fire inside every implementor's
    worktree — the exact failure mode row 2 of the story warns about.
    """

    def test_ticks_only_in_the_working_tree_are_not_reported(self):
        repo = self.repo()
        path = self.write_story(repo, "W-1", "ready", "    ")
        self.commit(repo, "file the story")
        self.write_story(repo, "W-1", "ready", "xxxx")
        self.assertTrue(path.read_text(encoding="utf-8").count("- [x]"), "the fixture must tick")

        self.assertQuietAbout(self.run_check(repo), "W-1")

    def test_the_staged_snapshot_reports_the_commit_being_written(self):
        """`--staged` reads the index, which is what the pre-commit hook needs.

        The hook's whole value is that it speaks at the moment the defect is created rather than
        three days later, and at that moment the ticks are staged and not yet committed.
        """
        repo = self.repo()
        self.write_story(repo, "W-1", "ready", "    ")
        self.commit(repo, "file the story")
        self.write_story(repo, "W-1", "ready", "xxxx")

        self.assertQuietAbout(self.run_check(repo, "--staged"), "W-1")
        self.git(repo, "add", "-A")
        self.assertReports(self.run_check(repo, "--staged"), "W-1")

    def test_the_commit_that_closes_the_story_is_quiet_in_the_staged_snapshot(self):
        """Tick and close in one commit and the hook says nothing, which is the whole point."""
        repo = self.repo()
        self.write_story(repo, "W-2", "ready", "    ")
        self.commit(repo, "file the story")
        self.write_story(repo, "W-2", "done", "xxxx")
        self.git(repo, "add", "-A")

        self.assertQuietAbout(self.run_check(repo, "--staged"), "W-2")


class TheBoardItReads(BoardCase):
    """What counts as a story, and what the checker does when it cannot read a board at all."""

    def test_files_that_are_not_stories_are_ignored(self):
        """The board's own README and template are not stories, and neither is a scratch note.

        The template carries an `id:` of its own, so a frontmatter test alone does not subsume the
        name list, and a note with no frontmatter is not a story however it is named.
        """
        repo = self.repo()
        stories = repo / "docs" / "stories"
        (stories / "README.md").write_text(
            "---\nid: README\nstatus: ready\n---\n\n## Acceptance\n\n- [x] generated\n",
            encoding="utf-8",
        )
        (stories / "_TEMPLATE.md").write_text(
            "---\nid: X-0\nstatus: ready\n---\n\n## Acceptance\n\n- [x] a template row\n",
            encoding="utf-8",
        )
        (stories / "notes.md").write_text(
            "# scratch\n\n## Acceptance\n\n- [x] not a story at all\n", encoding="utf-8"
        )
        self.commit(repo, "the board's furniture")

        result = self.run_check(repo)
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in ("README", "X-0", "notes"):
            self.assertQuietAbout(result, name)

    def test_a_board_that_cannot_be_read_is_not_reported_as_clean(self):
        """A repository with no commit has no committed board, and silence would read as *green*.

        Exit 2 is this repository's word for *the run was incomplete*, which is what this is.
        """
        repo = self.repo()
        self.write_story(repo, "A-1", "ready", "xxxx")

        result = self.run_check(repo)
        self.assertEqual(result.returncode, 2, f"{result.stdout}{result.stderr}")
        self.assertQuietAbout(result, "clean")


class TheExitCode(BoardCase):
    """A report is a report. The default exit code is what keeps it one."""

    def test_a_finding_does_not_fail_the_run(self):
        """Two of the three occurrences in this board's history were closed by the very next commit.

        Failing on those would have made the gate red for a state already being fixed — including on
        the commit that landed the completed work. So the default reports and returns success, and
        the strict mode exists for a caller that has decided otherwise.
        """
        repo = self.repo()
        self.write_story(repo, "A-1", "ready", "xxxx")
        self.commit(repo, "land the work, and forget to close the story")

        result = self.run_check(repo)
        self.assertEqual(result.returncode, 0, f"{result.stdout}{result.stderr}")
        self.assertReports(result, "A-1")

    def test_strict_fails_on_a_finding(self):
        repo = self.repo()
        self.write_story(repo, "A-1", "ready", "xxxx")
        self.commit(repo, "land the work, and forget to close the story")

        self.assertEqual(self.run_check(repo, "--strict").returncode, 1)

    def test_strict_succeeds_on_a_clean_board(self):
        repo = self.repo()
        self.write_story(repo, "R-1", "ready", "    ")
        self.write_story(repo, "M-70", "in-progress", "xxxxx")
        self.commit(repo, "an ordinary board")

        result = self.run_check(repo, "--strict")
        self.assertEqual(result.returncode, 0, f"{result.stdout}{result.stderr}")


if __name__ == "__main__":
    unittest.main(verbosity=2)
