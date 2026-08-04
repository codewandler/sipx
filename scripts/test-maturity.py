#!/usr/bin/env python3
"""Tests for `maturity.py`, against fixtures with known counts rather than the real sources.

The arithmetic is the whole product here, so it is asserted on data whose answers are written down in
the test. Running the generator against the real registry and eyeballing the table would prove only
that it produces a table.

The property that matters most is the one in `a_predicate_is_met_only_when_every_story_is_closed`: a
predicate's state must come from the board and nowhere else, because the alternative — a hand-kept
list of which predicates are met — is exactly the drift this generator exists to remove.
"""

import datetime
import importlib.util
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True

SCRIPT = pathlib.Path(__file__).resolve().parent / "maturity.py"

_SPEC = importlib.util.spec_from_file_location("maturity", SCRIPT)
maturity = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(maturity)


class TheLayerArithmetic(unittest.TestCase):
    """`partial` must never be folded into `implemented`, and the columns must add up."""

    ROWS = [
        {"layer": "media", "status": "implemented"},
        {"layer": "media", "status": "partial"},
        {"layer": "media", "status": "partial"},
        {"layer": "media", "status": "none"},
        {"layer": "media", "status": "syntax"},
        {"layer": "core", "status": "implemented"},
        {"layer": "core", "status": "n/a"},
    ]

    def test_each_status_is_counted_in_its_own_column(self):
        counts = maturity.layer_counts(self.ROWS)
        self.assertEqual(counts["media"]["implemented"], 1)
        self.assertEqual(counts["media"]["partial"], 2, "two partial rows are two, not one done")
        self.assertEqual(counts["media"]["none"], 1)

    def test_the_columns_sum_to_the_total(self):
        """A row that does not add up invites a reader to derive a percentage from what is shown."""
        for layer, counts in maturity.layer_counts(self.ROWS).items():
            named = (
                counts["implemented"] + counts["partial"] + counts["none"] + counts["other"]
            )
            self.assertEqual(
                named, counts["total"], f"{layer}'s columns do not account for every row"
            )

    def test_an_unknown_status_lands_in_other_rather_than_vanishing(self):
        counts = maturity.layer_counts([{"layer": "wire", "status": "something-new"}])
        self.assertEqual(counts["wire"]["other"], 1)
        self.assertEqual(counts["wire"]["total"], 1)

    def test_a_row_with_no_layer_is_still_counted(self):
        """Silently dropping a row would understate the distance, which is the wrong direction."""
        counts = maturity.layer_counts([{"status": "partial"}])
        self.assertEqual(counts["?"]["total"], 1)


class ThePillarArithmetic(unittest.TestCase):
    FOUND = {
        "A-1": {"status": "done", "pillar": "Build"},
        "A-2": {"status": "ready", "pillar": "Build"},
        "A-3": {"status": "backlog", "pillar": "Media"},
        "A-4": {"status": "blocked", "pillar": "Media"},
        "A-5": {"status": "in-progress", "pillar": "Media"},
        "A-6": {"status": "done", "pillar": "Media"},
    }

    def test_blocked_counts_as_open(self):
        """A story parked on a dependency is distance, not progress."""
        per_pillar, _ = maturity.pillar_counts(self.FOUND)
        self.assertEqual(per_pillar["Media"], 3, "backlog, blocked and in-progress are all open")

    def test_done_is_counted_separately_and_not_as_open(self):
        per_pillar, done = maturity.pillar_counts(self.FOUND)
        self.assertEqual(done, 2)
        self.assertEqual(per_pillar["Build"], 1)


class ThePredicateRule(unittest.TestCase):
    """The rule that keeps this report from becoming a second, drifting source of truth."""

    def test_a_predicate_is_met_only_when_every_story_is_closed(self):
        predicate = maturity.Predicate(1, "example", "computed")
        found = {"X-1": {"status": "done", "predicate": "1"}, "X-2": {"status": "ready", "predicate": "1"}}
        open_stories, declared = maturity.predicate_state(predicate, found)
        self.assertEqual(open_stories, ["X-2"])
        self.assertEqual(declared, ["X-1", "X-2"])
        self.assertEqual(maturity.predicate_row(predicate, found)[0], "open")

        found["X-2"]["status"] = "done"
        open_stories, _ = maturity.predicate_state(predicate, found)
        self.assertEqual(open_stories, [], "every story closed means the predicate is met")
        self.assertEqual(maturity.predicate_row(predicate, found)[0], "met")

    def test_a_computed_predicate_no_story_declares_is_unknown_not_met(self):
        """Renaming or deleting a story must not silently satisfy a predicate.

        This is the failure mode that would make the whole report worthless. It used to be a blocker
        list pointing at a story that no longer existed; now that the stories declare the predicate,
        the same hole is a predicate nothing declares — deleting the last story that named one would
        otherwise look exactly like finishing it.
        """
        predicate = maturity.Predicate(1, "example", "computed")
        state, waiting = maturity.predicate_row(predicate, {"X-1": {"status": "ready"}})
        self.assertEqual(state, "**unknown**")
        self.assertIn("no story declares", waiting)

    def test_an_attested_predicate_needs_no_story(self):
        """Predicate 6 is the case: an attestation nothing contradicts is not an unknown."""
        predicate = maturity.Predicate(6, "example", "attested")
        self.assertEqual(maturity.predicate_row(predicate, {})[0], "met (attested)")

    def test_a_story_declaring_a_predicate_that_does_not_exist_is_an_error(self):
        """A typo must not be dropped, because a dropped declaration reports as progress.

        The old shape of this test read `ALPHA` for stories that were not on the board. The direction
        has reversed with the source of the association: the board now names predicates, so what can
        be wrong is a story naming a predicate the roadmap does not have.
        """
        with self.assertRaises(SystemExit) as caught:
            maturity.predicate_stories({"X-1": {"status": "ready", "predicate": "8"}})
        self.assertIn("no alpha predicate 8", str(caught.exception))

    def test_a_predicate_field_that_is_not_a_number_is_an_error(self):
        """Malformed frontmatter gets a diagnostic and a non-zero exit, never a traceback."""
        with self.assertRaises(SystemExit) as caught:
            maturity.story_predicates("X-1", {"predicate": "three"})
        self.assertIn("not a predicate number", str(caught.exception))

    def test_a_story_can_declare_more_than_one_predicate(self):
        """A defect can falsify two predicates, and forcing a filer to pick one would hide the other."""
        self.assertEqual(maturity.story_predicates("X-1", {"predicate": "[3, 7]"}), (3, 7))
        self.assertEqual(maturity.story_predicates("X-1", {"predicate": "3"}), (3,))
        self.assertEqual(maturity.story_predicates("X-1", {"predicate": ""}), ())
        self.assertEqual(maturity.story_predicates("X-1", {}), ())

    def test_every_predicate_the_board_declares_exists(self):
        """The real board, not a fixture: a `predicate:` typo would otherwise be silently dropped."""
        maturity.predicate_stories(maturity.stories())

    def test_every_computed_predicate_is_declared_by_at_least_one_story(self):
        """The real board: a computed predicate reads **unknown** until some story claims it.

        This is what makes the mechanism populated rather than merely available. Without it the
        literal could have been deleted and every computed predicate would read `unknown` — honest,
        but useless, and nothing would have said so.
        """
        declared = maturity.predicate_stories(maturity.stories())
        for predicate in maturity.ALPHA:
            if predicate.kind == "computed":
                self.assertTrue(
                    declared.get(predicate.number),
                    f"no story declares `predicate: {predicate.number}`, so it reports unknown",
                )

    def test_an_attested_predicate_says_why_it_is_not_computed(self):
        """An attestation reported as a measurement is the one thing this file must not do."""
        for predicate in maturity.ALPHA:
            if predicate.kind == "attested":
                self.assertTrue(
                    predicate.detail,
                    f"predicate {predicate.number} is attested and must say why",
                )


class TheAnnouncementPredicateRule(unittest.TestCase):
    """The beta gate is separate from v1 and owns its story associations the same safe way."""

    def test_an_open_announcement_story_holds_its_predicate_open(self):
        predicate = maturity.Predicate(2, "Shell proof", "computed")
        found = {
            "P-11": {"status": "done", "announcement": "2"},
            "P-13": {"status": "backlog", "announcement": "[2, 3]"},
        }
        state, waiting = maturity.predicate_row(
            predicate,
            found,
            predicates=maturity.BETA,
            field=maturity.ANNOUNCEMENT_FIELD,
            gate="beta-announcement",
        )
        self.assertEqual(state, "open")
        self.assertEqual(waiting, "`P-13`")

    def test_an_invalid_announcement_number_is_an_error(self):
        with self.assertRaises(SystemExit) as caught:
            maturity.predicate_stories(
                {"A-1": {"status": "backlog", "announcement": "7"}},
                predicates=maturity.BETA,
                field=maturity.ANNOUNCEMENT_FIELD,
                gate="beta-announcement",
            )
        self.assertIn("no beta-announcement predicate 7", str(caught.exception))

    def test_an_undeclared_computed_announcement_predicate_is_unknown(self):
        predicate = maturity.Predicate(3, "Interop", "computed")
        state, waiting = maturity.predicate_row(
            predicate,
            {},
            predicates=(predicate,),
            field=maturity.ANNOUNCEMENT_FIELD,
            gate="beta-announcement",
        )
        self.assertEqual(state, "**unknown**")
        self.assertIn("announcement: 3", waiting)

    def test_announcement_integrity_reopens_with_any_alpha_predicate(self):
        alpha = (maturity.Predicate(1, "Alpha integrity", "computed"),)
        found = {"X-1": {"status": "ready", "predicate": "1"}}
        state, waiting = maturity.announcement_predicate_row(
            maturity.BETA[0], found, alpha=alpha
        )
        self.assertEqual(state, "open")
        self.assertIn("alpha predicate 1", waiting)

        found["X-1"]["status"] = "done"
        state, waiting = maturity.announcement_predicate_row(
            maturity.BETA[0], found, alpha=alpha
        )
        self.assertEqual((state, waiting), ("met", "—"))

    def test_every_real_announcement_declaration_names_a_real_predicate(self):
        maturity.predicate_stories(
            maturity.stories(),
            predicates=maturity.BETA,
            field=maturity.ANNOUNCEMENT_FIELD,
            gate="beta-announcement",
        )


class APredicateSeesEveryStoryFiledAgainstIt(unittest.TestCase):
    """`X-42`: the association is declared by the story, so filing one cannot be forgotten.

    Predicate 3 read **met** while `X-39`, `X-40` and `X-41` were open and each described that
    predicate failing — a gate step that cannot pass, a test that fails because the machine was busy,
    and a step that prints a defect and exits 0. All three were filed in one session against a list
    that lived in a Python literal here, which a filer had no reason to open. The list was the defect.
    """

    def test_an_open_story_that_declares_a_predicate_holds_it_open(self):
        """The `X-42` case in miniature: two stories declare predicate 3, one of them is still open."""
        predicate = maturity.Predicate(3, "A red gate means a defect", "computed")
        found = {
            "X-28": {"status": "done", "predicate": "3"},
            "X-39": {"status": "ready", "predicate": "3"},
            "S-27": {"status": "ready", "predicate": "4"},
        }
        open_stories, declared = maturity.predicate_state(predicate, found)
        self.assertEqual(
            open_stories,
            ["X-39"],
            "a story declaring predicate 3 and still open must keep predicate 3 from reading met",
        )
        self.assertEqual(
            declared,
            ["X-28", "X-39"],
            "and a story declaring a different predicate is not this one's business",
        )


class TheStatusVocabulary(unittest.TestCase):
    """One definition of each status word, in the file that defines it (`X-38` rework).

    The report asserted "`implemented` now means the code exists in a crate the shipped application
    depends on". That was false — RFC 8996 is `implemented` citing `docs/specs/sip-tls.md` and no crate
    — and it gave a load-bearing word a second meaning conflicting with the schema table that
    `rfc-report.py` actually enforces. Two definitions across the two documents a reader consults is the
    drift this repository keeps closing, so the report now reads the definition instead of restating it.
    """

    def test_the_definition_is_read_from_the_schema_table(self):
        self.assertEqual(
            maturity.status_definition("implemented"),
            "Behaviour present and tested for the roles listed",
        )

    def test_a_word_the_schema_does_not_define_is_an_error(self):
        """A reader that silently returned nothing would render an empty definition."""
        with self.assertRaises(SystemExit):
            maturity.status_definition("invented")

    def test_the_report_quotes_the_schema_rather_than_redefining_it(self):
        self.assertIn(maturity.status_definition("implemented"), maturity.render())

    def test_the_report_does_not_redefine_implemented(self):
        """The false sentence, as a test.

        Deliberately asserted on the report and **not** on the registry. RFC 8996 is the row the false
        sentence was caught by — `implemented` on the evidence of `docs/specs/sip-tls.md` and no crate —
        and `X-43` is open to re-evidence exactly that row and to decide whether `implemented` should
        require a `crates/` path at all. A test that pinned 8996's evidence, or that required some
        spec-only row to exist, would fail the moment `X-43` lands and would be a landmine in someone
        else's story. What this report must not do is define the word, whatever the registry holds.
        """
        report = maturity.render()
        self.assertNotIn("the code exists in a crate", report)
        self.assertNotIn("`implemented` now means", report)


class TheReport(unittest.TestCase):
    def test_the_committed_report_is_what_the_sources_say(self):
        """`--check`'s subject, asserted here too so a stale report fails the suite as well."""
        self.assertEqual(
            maturity.existing().strip(),
            maturity.render().strip(),
            "docs/maturity.md has drifted; run ./scripts/maturity.py",
        )

    def test_the_report_states_its_blind_spot(self):
        """The report must name the limit of its own reachability claim.

        This used to assert the string `unverified against callers`, which was the caveat `X-30`
        through `X-37` left standing. `X-38` resolved it by definition — the surface *is* what the
        shipped application uses — and the replacement bullet quotes the old phrase while saying it is
        gone, so the original assertion went on passing against text that no longer made the claim.
        A test that cannot fail for the reason it was written is the defect `X-36` was filed over, so
        it is pinned to the new limit instead: one application, entered per crate.
        """
        text = maturity.render()
        self.assertIn("What this cannot see", text)
        self.assertIn(
            "one application's opinion",
            text,
            "the report must say that its reachable surface is one application's, not everyone's",
        )
        self.assertIn(
            "per crate",
            text,
            "the surface is entered per crate, so a supported module nothing names is not caught",
        )

    def test_predicate_one_reports_the_application_as_its_basis(self):
        """`X-38`: predicate 1 stopped being an attestation and says what it is computed from.

        Both halves matter. If it went back to `attested` the report would be claiming less than the
        gate now enforces; if it claimed to be computed without naming the application and the checker,
        a reader could not tell what the computation was over — and the definition *is* the result here.
        """
        predicate = next(item for item in maturity.ALPHA if item.number == 1)
        self.assertEqual(predicate.kind, "computed")
        self.assertIn(maturity.SURFACE_APPLICATION, predicate.detail)
        self.assertIn(maturity.SURFACE_CHECKER, predicate.detail)

    def test_a_computed_predicate_is_not_labelled_an_attestation(self):
        """The note over predicate 1 must not call its mechanical check an attestation."""
        text = maturity.render()
        self.assertNotIn("Predicate 1 is attested", text)
        self.assertIn("Predicate 1 is computed", text)

    def test_the_resolved_caveat_is_gone_from_the_layer_table(self):
        """The four layers that carried `**no**` must no longer say a caller has not been found."""
        text = maturity.render()
        self.assertNotIn("| **no** |", text)
        self.assertIn("Reachability basis", text)

    def test_no_aggregate_percentage_is_reported(self):
        """One number over unlike layers is the metric this story exists to refuse."""
        self.assertNotIn("%", maturity.render())

    def test_the_reachability_layers_match_the_checker(self):
        """If `rfc-report.py` widens its scope, the caveat here is wrong until it follows."""
        checker = (
            pathlib.Path(__file__).resolve().parent / "rfc-report.py"
        ).read_text()
        for layer in maturity.REACHABILITY_CHECKED:
            self.assertIn(
                f'"{layer}"',
                checker,
                f"maturity.py claims {layer} is reachability-checked and rfc-report.py does not",
            )


class TheCheckIsSatisfiableByTheCommitThatMovesTheBoard(unittest.TestCase):
    """`X-39`: the gate's `maturity` step must be able to pass in the commit that moves the board.

    Asserted against a miniature repository and not this one, because the subject is what git history
    says at a particular commit and the real history cannot be arranged. The fixture holds the three
    sources `render()` reads — the registry, the status schema and the board — plus a copy of
    `maturity.py`, so this is the real generator over data whose answers are written down here.

    Before the fix, both `..._is_gate_green_without_a_second_commit` tests failed the same way and for
    the reason the story describes: `maturity.py` runs before `git commit`, so the day row it wrote was
    one short of the count that the commit carrying it created, and `--check` reported drift in a tree
    where nothing was wrong. That made the step red in most commits and never for a defect.
    """

    STORY = "---\nid: {id}\ntitle: {id}\npillar: Build\nstatus: {status}\npredicate: 3\n---\n\n# {id}\n"

    REGISTRY = '[[rfc]]\nnumber = 3261\nlayer = "core"\nstatus = "implemented"\n'

    #: Only the row `status_definition` reads. The real schema table carries more prose; what the
    #: reader needs is a two-cell row whose first cell is the status word in backticks.
    SCHEMA = (
        "| Status | Meaning |\n|---|---|\n"
        "| `implemented` | Behaviour present and tested for the roles listed. |\n"
    )

    def git(self, repo, *args, date=None, check=True):
        """A git command in the fixture, with an identity so `commit` works on any machine.

        `date` back-dates both the author and the committer date. The day rows are read from `%ad`, so
        a fixture that needs a history *not* dated today — the only way to observe a day with no story
        activity — sets it.

        `check=False` is for the one command expected to fail: an add/add merge conflict, which is how
        a story filed on two lines of history is arranged.
        """
        env = dict(os.environ)
        if date:
            env["GIT_AUTHOR_DATE"] = date
            env["GIT_COMMITTER_DATE"] = date
        return subprocess.run(
            [
                "git",
                "-c",
                "user.name=maturity test",
                "-c",
                "user.email=maturity@example.invalid",
                "-c",
                "commit.gpgsign=false",
                *args,
            ],
            cwd=repo,
            capture_output=True,
            text=True,
            check=check,
            env=env,
        )

    def commit(self, repo, message, date=None):
        self.git(repo, "add", "-A")
        self.git(repo, "commit", "-q", "--no-verify", "-m", message, date=date)

    def stage(self, repo, *paths):
        self.git(repo, "add", "--", *paths)

    def commit_staged(self, repo, message, date=None, amend=False):
        args = ["commit", "-q", "--no-verify", "-m", message]
        if amend:
            args.extend(["--amend", "--no-edit"])
        self.git(repo, *args, date=date)

    def clean_checkout(self, repo):
        checkout = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, checkout, ignore_errors=True)
        self.git(repo.parent, "clone", "-q", "--no-local", str(repo), str(checkout))
        return checkout

    def run_maturity(self, repo, *args, source_date_epoch=None):
        env = dict(os.environ)
        if source_date_epoch is not None:
            env["SOURCE_DATE_EPOCH"] = str(source_date_epoch)
        return subprocess.run(
            [sys.executable, str(repo / "scripts" / "maturity.py"), *args],
            cwd=repo,
            capture_output=True,
            text=True,
            check=False,
            env=env,
        )

    def day_row(self, repo, day=None):
        """One row of the *Discovery versus closure* table, defaulting to today."""
        day = day or datetime.date.today().isoformat()
        for line in (repo / "docs" / "maturity.md").read_text().splitlines():
            if line.startswith(f"| {day} |"):
                return line
        return None

    def fixture(self, date=None):
        """A repository whose report is committed, current, and green — the state before the defect.

        `date` back-dates the whole history, which is how a test arranges a today with nothing in it.
        """
        repo = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, repo, ignore_errors=True)
        (repo / "scripts").mkdir()
        (repo / "docs" / "rfc").mkdir(parents=True)
        (repo / "docs" / "stories").mkdir(parents=True)
        shutil.copy(SCRIPT, repo / "scripts" / "maturity.py")
        (repo / "docs" / "rfc" / "registry.toml").write_text(self.REGISTRY)
        (repo / "docs" / "rfc" / "README.md").write_text(self.SCHEMA)
        for story in ("X-1", "X-2"):
            self.write_story(repo, story, "ready")

        self.git(repo, "init", "-q")
        self.commit(repo, "seed the board", date=date)
        self.assertEqual(self.run_maturity(repo).returncode, 0, "the fixture must generate")
        self.commit(repo, "the report of that board", date=date)
        green = self.run_maturity(repo, "--check")
        self.assertEqual(
            green.returncode,
            0,
            f"the fixture must start green or these tests prove nothing: {green.stderr}",
        )
        return repo

    def write_story(self, repo, story_id, status):
        (repo / "docs" / "stories" / f"{story_id}-a-story.md").write_text(
            self.STORY.format(id=story_id, status=status)
        )

    def test_a_commit_that_files_a_story_is_gate_green_without_a_second_commit(self):
        """The reproduction from the story's Acceptance, as a test.

        File a story, regenerate, commit both, check. `Filed` for today is created by the very commit
        that carries the report, so before the fix no ordering of the two could satisfy `--check`.
        """
        repo = self.fixture()
        self.write_story(repo, "X-3", "ready")
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        before_commit = self.run_maturity(repo, "--check")
        self.assertEqual(before_commit.returncode, 0, before_commit.stderr)
        self.commit(repo, "file X-3, with the report")

        checked = self.run_maturity(repo, "--check")
        self.assertEqual(
            checked.returncode,
            0,
            "a commit that files a story must be able to carry a report of itself: "
            f"{checked.stderr}{self.day_row(repo)}",
        )

    def test_a_commit_that_closes_a_story_is_gate_green_without_a_second_commit(self):
        """The other half, and the one that bit `main` twice on 2026-07-30.

        `Closed` is a `status: done` line appearing in a committed diff, which is the same
        unobtainable shape as `Filed`. Closing a story also moves the pillar totals and predicate 3's
        state, and those come from the story files rather than from history — so this test is green on
        those counts either way, and what it isolates is the day row.
        """
        repo = self.fixture()
        self.write_story(repo, "X-2", "done")
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        before_commit = self.run_maturity(repo, "--check")
        self.assertEqual(before_commit.returncode, 0, before_commit.stderr)
        self.commit(repo, "close X-2, with the report")

        checked = self.run_maturity(repo, "--check")
        self.assertEqual(
            checked.returncode,
            0,
            "a commit that closes a story must be able to carry a report of itself: "
            f"{checked.stderr}{self.day_row(repo)}",
        )

    def test_a_report_that_was_not_regenerated_is_still_red(self):
        """The drift `--check` was built to catch, which the fix must not trade away.

        This is the 2026-07-30 case where `main` was red for real: `S-25` closed, the aggregates
        moved, nothing re-ran the script. A fix tolerant enough to miss it would be worse than the
        flapping, because the report is linked as a measurement.
        """
        repo = self.fixture()
        self.write_story(repo, "X-1", "done")
        self.commit(repo, "close X-1 and forget the report")

        checked = self.run_maturity(repo, "--check")
        self.assertEqual(checked.returncode, 1, "an unregenerated report is drift")
        self.assertIn("drifted", checked.stderr)

    def test_a_committed_fact_missing_from_the_event_journal_is_still_red(self):
        """Strict drift is carried by the day source itself, not only by board aggregates.

        The extra closing line is outside frontmatter, so the board and predicate tables do not move.
        Git history still contains the closing fact the discovery table measures. If reconciliation
        with the committed journal is removed, this test is green and the missing event is silent.
        """
        repo = self.fixture()
        path = repo / "docs" / "stories" / "X-1-a-story.md"
        path.write_text(path.read_text() + "\nstatus: done\n")
        self.commit(repo, "record a closing fact and forget the report")

        checked = self.run_maturity(repo, "--check")
        self.assertEqual(checked.returncode, 1, "an event absent from the journal is drift")
        self.assertIn("drifted", checked.stderr)

    def test_an_edited_report_is_still_red(self):
        """Hand-editing the generated region must fail, day rows included.

        The narrower risk of reading a pre-commit snapshot: if the day row were merely tolerated, a
        wrong number in it would pass while it was today's. It is not tolerated — the source changed,
        and the comparison is as strict as it ever was.
        """
        repo = self.fixture()
        report = repo / "docs" / "maturity.md"
        row = self.day_row(repo)
        self.assertIsNotNone(row, "the fixture files its stories today, so today has a row")
        report.write_text(report.read_text().replace(row, row.replace("| 2 |", "| 9 |", 1)))
        self.commit(repo, "edit the table by hand")

        checked = self.run_maturity(repo, "--check")
        self.assertEqual(checked.returncode, 1, "a hand-edited day row is drift")

    def test_a_story_filed_already_closed_is_counted_by_both_halves(self):
        """An added file's staged diff and committed diff must count the same closing line.

        Committing a new file shows its whole body as `+` lines, so history counts a story filed
        already `done` as both filed and closed. The index half must do the same or the two halves of
        the union disagree and the row moves under its own commit.
        """
        repo = self.fixture()
        self.write_story(repo, "X-3", "done")
        self.stage(repo, "docs/stories/X-3-a-story.md")
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        before = self.day_row(repo)
        today = datetime.date.today().isoformat()
        self.assertEqual(before, f"| {today} | 3 | 1 | -2 |", "filed and closed by the same file")

        self.commit(repo, "file X-3 already closed, with the report")
        self.assertEqual(self.run_maturity(repo, "--check").returncode, 0)
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        self.assertEqual(self.day_row(repo), before)

    def test_a_clean_tree_reports_committed_history_alone(self):
        """The arithmetic of today's row, pinned on a clean tree."""
        repo = self.fixture()
        today = datetime.date.today().isoformat()
        self.assertEqual(
            self.day_row(repo),
            f"| {today} | 2 | 0 | -2 |",
            "two stories filed today, none closed",
        )

    def test_a_day_with_no_story_activity_gets_no_row_at_all(self):
        """The zero-guard, which nothing else here can observe.

        `days` is the union of the two counters' keys and `Counter[key] += 0` *creates* the key, so an
        unguarded bump for today prints a phantom `| today | 0 | 0 | +0 |` row on every clean tree —
        a day the table says nothing happened on, in a table whose whole subject is what happened per
        day, and a fresh red gate every midnight. That is the failure class this story exists to
        remove, reintroduced by the fix that removes it.

        **Every other test in this class files or closes something today**, so today is already a key
        in `filed` and the phantom row is invisible to all of them — including the earlier version of
        this test, whose `assertNotIn` could not fire and whose docstring claimed this property
        anyway. That is the `X-36` shape: a test that cannot detect the reversal of the invariant it is
        named for. This one back-dates the entire history instead, so today is genuinely empty, and it
        goes red the moment the guard in `discovery_rate` is deleted.
        """
        repo = self.fixture(date="2020-01-02T03:04:05")
        report = (repo / "docs" / "maturity.md").read_text()

        self.assertIsNone(
            self.day_row(repo),
            "nothing was filed or closed today, so the table must not invent a row for it",
        )
        self.assertIn("| 2020-01-02 | 2 | 0 | -2 |", report, "the day that did have activity")
        self.assertNotIn("| 0 | 0 | +0 |", report, "and no day with nothing in it")
        self.assertEqual(self.run_maturity(repo, "--check").returncode, 0)

    def test_a_file_with_no_story_frontmatter_is_not_a_filed_story(self):
        """A scratch note in the story directory is not a story filed that day.

        Decided deliberately, because the alternative is worse than merely noisy. Counting it by name
        made `--check` red on a tree whose report was correct — this story's own failure mode from a
        new direction — and it also made the report *green in the tree holding the scratch file and
        red on a clean checkout of the same commit*, because the file is never committed. Local green
        with CI red is the `X-22` failure class, which is the one this repository's gate section exists
        to prevent, so the name rule had to go.

        `story_fields` decides it now, the same test `stories()` applies, and the frontmatter half of
        that test is not sufficient on its own: the board's `_TEMPLATE.md` carries an `id:` too.
        """
        repo = self.fixture()
        before = self.day_row(repo)
        (repo / "docs" / "stories" / "notes.md").write_text("scratch, not a story\n")

        checked = self.run_maturity(repo, "--check")
        self.assertEqual(
            checked.returncode,
            0,
            f"a scratch file must not make a correct report look drifted: {checked.stderr}",
        )
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        self.assertEqual(self.day_row(repo), before, "and must not move the count")

    def test_a_closing_line_with_a_trailing_space_is_read_the_same_by_both_halves(self):
        """One reader for the closing line, asserted on the permutation that broke.

        The halves used to disagree: history matched `startswith`, the working tree matched equality.
        A story filed already `done` with a trailing space on that line was therefore closed according
        to history and open according to the working tree, so the row moved across its own commit and
        the flap survived on malformed frontmatter. `M-31`'s shape, and `M-31`'s fix.

        The board agrees the story is closed — `story_fields` strips values — so counting it is also
        the right answer and not merely the consistent one.
        """
        repo = self.fixture()
        (repo / "docs" / "stories" / "X-3-a-story.md").write_text(
            "---\nid: X-3\ntitle: X-3\npillar: Build\nstatus: done \n---\n\n# X-3\n"
        )
        self.stage(repo, "docs/stories/X-3-a-story.md")
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        before = self.day_row(repo)
        today = datetime.date.today().isoformat()
        self.assertEqual(before, f"| {today} | 3 | 1 | -2 |", "a trailing space still closes a story")

        self.commit(repo, "file X-3 closed, with a trailing space, with the report")
        checked = self.run_maturity(repo, "--check")
        self.assertEqual(checked.returncode, 0, f"the row must not move: {checked.stderr}")
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        self.assertEqual(self.day_row(repo), before)

    def test_every_non_reserved_board_markdown_file_is_a_story(self):
        """The current board has no ambiguous Markdown file outside its reserved generated files.

        History and pending snapshots now both inspect content, so this is no longer needed to paper
        over a filename/content asymmetry. It remains a useful board-shape assertion: a committed
        scratch note is consistently a non-story, but keeping one here would still be confusing.
        """
        for path in sorted(maturity.STORIES.glob("*.md")):
            self.assertEqual(
                path.name not in maturity.NOT_STORIES,
                maturity.story_fields(path) is not None,
                f"{path.name} is a story by one rule and not the other, so the day row for the "
                f"commit that added it moves under that commit",
            )

    def test_the_index_half_is_what_holds_the_row_still(self):
        """The mechanism directly: the count must not change when the index is committed.

        Asserted as an equality across the commit rather than only through `--check`, because that is
        the property the fix rests on — `git commit` relocates a fact from the index to
        history and the union is unmoved. If this drifts apart again, `--check` going red is a symptom
        and this is the cause.
        """
        repo = self.fixture()
        self.write_story(repo, "X-3", "ready")
        self.write_story(repo, "X-2", "done")
        self.stage(repo, "docs/stories/X-3-a-story.md", "docs/stories/X-2-a-story.md")
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        before = self.day_row(repo)
        self.commit(repo, "file one, close one, with the report")
        self.assertEqual(self.run_maturity(repo).returncode, 0)

        self.assertEqual(
            self.day_row(repo),
            before,
            "committing the change must not move the day row it belongs to",
        )
        today = datetime.date.today().isoformat()
        self.assertEqual(before, f"| {today} | 3 | 1 | -2 |")

    def test_selective_commit_ignores_unstaged_and_untracked_stories_in_both_trees(self):
        """The report describes the index, even when the worktree contains a different board.

        X-3 and the report are the selective commit. X-2's later close and untracked X-4 remain only
        in the originating worktree. Both that dirty tree and a clean checkout of the commit must
        accept the same report; counting either excluded story creates a local/CI disagreement.
        """
        repo = self.fixture()
        self.write_story(repo, "X-3", "ready")
        self.stage(repo, "docs/stories/X-3-a-story.md")
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        self.stage(repo, "docs/maturity.md")

        self.write_story(repo, "X-3", "done")
        self.write_story(repo, "X-2", "done")
        self.write_story(repo, "X-4", "ready")
        local = self.run_maturity(repo, "--check")
        self.assertEqual(local.returncode, 0, local.stderr)

        self.commit_staged(repo, "selectively file X-3 with its report")
        self.assertTrue((repo / "docs" / "stories" / "X-4-a-story.md").exists())

        checkout = self.clean_checkout(repo)
        clean = self.run_maturity(checkout, "--check")
        self.assertEqual(clean.returncode, 0, clean.stderr)
        self.assertFalse((checkout / "docs" / "stories" / "X-4-a-story.md").exists())

    def test_a_staged_non_story_markdown_file_is_not_a_fact_before_or_after_commit(self):
        """History and the staged snapshot must apply the same definition of a story."""
        repo = self.fixture()
        before = self.day_row(repo)
        path = repo / "docs" / "stories" / "notes.md"
        path.write_text("scratch, not story frontmatter\n")
        self.stage(repo, "docs/stories/notes.md")
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        self.assertEqual(self.day_row(repo), before)
        self.stage(repo, "docs/maturity.md")
        self.commit_staged(repo, "commit a non-story note")

        checkout = self.clean_checkout(repo)
        checked = self.run_maturity(checkout, "--check")
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertEqual(self.day_row(checkout), before)

    def test_selective_story_deletion_is_stable_in_a_clean_checkout(self):
        repo = self.fixture()
        self.git(repo, "rm", "-q", "docs/stories/X-2-a-story.md")
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        self.stage(repo, "docs/maturity.md")
        self.commit_staged(repo, "delete X-2 with its report")

        checkout = self.clean_checkout(repo)
        checked = self.run_maturity(checkout, "--check")
        self.assertEqual(checked.returncode, 0, checked.stderr)

    def test_selective_story_rename_is_not_a_new_filing(self):
        repo = self.fixture()
        before = self.day_row(repo)
        self.git(
            repo,
            "mv",
            "docs/stories/X-2-a-story.md",
            "docs/stories/X-2-renamed-story.md",
        )
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        self.assertEqual(self.day_row(repo), before)
        self.stage(repo, "docs/maturity.md")
        self.commit_staged(repo, "rename X-2 with its report")

        checkout = self.clean_checkout(repo)
        checked = self.run_maturity(checkout, "--check")
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertEqual(self.day_row(checkout), before)

    def test_staging_only_the_report_while_a_story_is_unstaged_is_an_error(self):
        """A report-only commit cannot claim facts deliberately absent from its snapshot."""
        repo = self.fixture()
        self.write_story(repo, "X-3", "ready")
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        self.stage(repo, "docs/maturity.md")

        checked = self.run_maturity(repo, "--check")
        self.assertNotEqual(checked.returncode, 0)
        self.assertIn("report is staged while story changes are not", checked.stderr)

    def test_staged_journal_keeps_its_generation_day_when_the_generator_crosses_midnight(self):
        """A later generator invocation must reuse the staged journal, not re-date its facts."""
        repo = self.fixture(date="2020-01-02T03:04:05+00:00")
        first_day = "2030-01-02"
        next_day = "2030-01-03"
        first_epoch = int(datetime.datetime(2030, 1, 2, tzinfo=datetime.timezone.utc).timestamp())
        next_epoch = int(datetime.datetime(2030, 1, 3, tzinfo=datetime.timezone.utc).timestamp())
        self.write_story(repo, "X-3", "ready")
        self.stage(repo, "docs/stories/X-3-a-story.md")
        self.assertEqual(self.run_maturity(repo, source_date_epoch=first_epoch).returncode, 0)
        before = self.day_row(repo, first_day)
        self.assertIsNotNone(before, "SOURCE_DATE_EPOCH must control the generator's event day")
        self.assertIsNone(self.day_row(repo, next_day))
        self.stage(repo, "docs/maturity.md")

        after_midnight = self.run_maturity(repo, "--check", source_date_epoch=next_epoch)
        self.assertEqual(after_midnight.returncode, 0, after_midnight.stderr)
        self.commit_staged(repo, "file X-3 after midnight", date=f"{next_day}T00:01:00+00:00")

        actual_day = self.git(repo, "log", "-1", "--format=%ad", "--date=short").stdout.strip()
        self.assertEqual(actual_day, next_day)
        checkout = self.clean_checkout(repo)
        checked = self.run_maturity(checkout, "--check")
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertEqual(self.day_row(checkout, first_day), before)
        self.assertIsNone(self.day_row(checkout, next_day))

    def test_generation_day_survives_amend_with_the_retained_old_author_date(self):
        """An amend keeps the old author date, but a newly staged fact keeps its generation day."""
        old_day = "2020-01-02"
        repo = self.fixture(date=f"{old_day}T03:04:05+00:00")
        today = datetime.date.today().isoformat()
        self.write_story(repo, "X-3", "ready")
        self.stage(repo, "docs/stories/X-3-a-story.md")
        self.assertEqual(self.run_maturity(repo).returncode, 0)
        before = self.day_row(repo, today)
        self.stage(repo, "docs/maturity.md")
        self.commit_staged(repo, "ignored for amend", amend=True)

        actual_day = self.git(repo, "log", "-1", "--format=%ad", "--date=short").stdout.strip()
        self.assertEqual(actual_day, old_day)
        checkout = self.clean_checkout(repo)
        checked = self.run_maturity(checkout, "--check")
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertEqual(self.day_row(checkout, today), before)

    def replace_journal(self, repo, data):
        path = repo / "docs" / "maturity.md"
        lines = path.read_text().splitlines()
        current_line = next(line for line in lines if line.startswith("<!-- maturity-event-days: "))
        current = json.loads(current_line[len("<!-- maturity-event-days: ") : -len(" -->")])
        if "basis" not in data:
            data = {"basis": current["basis"], **data}
        replacement = "<!-- maturity-event-days: " + json.dumps(data) + " -->"
        path.write_text(
            "\n".join(
                replacement if line.startswith("<!-- maturity-event-days: ") else line
                for line in lines
            )
            + "\n"
        )
        self.commit(repo, "commit a malformed event journal")

    def test_malformed_event_journals_are_rejected(self):
        cases = {
            "unexpected top-level key": {
                "filed": {"2020-01-02": 2},
                "closed": {},
                "extra": {},
            },
            "invalid date": {"filed": {"not-a-date": 2}, "closed": {}},
            "zero count": {"filed": {"2020-01-02": 0}, "closed": {}},
            "negative count": {"filed": {"2020-01-02": -1}, "closed": {}},
            "string count": {"filed": {"2020-01-02": "2"}, "closed": {}},
        }
        for label, data in cases.items():
            with self.subTest(label=label):
                repo = self.fixture(date="2020-01-02T03:04:05+00:00")
                self.replace_journal(repo, data)
                checked = self.run_maturity(repo, "--check")
                self.assertNotEqual(checked.returncode, 0)
                self.assertIn("invalid event-date journal", checked.stderr)

    def test_a_journal_claiming_more_facts_than_the_snapshot_is_rejected(self):
        repo = self.fixture(date="2020-01-02T03:04:05+00:00")
        self.replace_journal(repo, {"filed": {"2020-01-02": 3}, "closed": {}})

        checked = self.run_maturity(repo, "--check")
        self.assertNotEqual(checked.returncode, 0)
        self.assertIn("journal records 3 filed facts", checked.stderr)

    def test_rewriting_journal_and_table_dates_without_changing_totals_is_rejected(self):
        """Date attribution is tied to fact identities, not accepted because totals happen to match."""
        repo = self.fixture(date="2020-01-02T03:04:05+00:00")
        path = repo / "docs" / "maturity.md"
        text = path.read_text()
        text = text.replace('"2020-01-02":2', '"2020-01-03":2')
        text = text.replace("| 2020-01-02 | 2 | 0 | -2 |", "| 2020-01-03 | 2 | 0 | -2 |")
        path.write_text(text)
        self.commit(repo, "rewrite event attribution without changing totals")

        checked = self.run_maturity(repo, "--check")
        self.assertNotEqual(checked.returncode, 0)
        self.assertIn("event-date journal basis", checked.stderr)

    def shallow_checkout(self, repo, depth=1):
        """A depth-limited clone — what `actions/checkout` produces without `fetch-depth: 0`."""
        checkout = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, checkout, ignore_errors=True)
        self.git(
            repo.parent,
            "clone",
            "-q",
            "--no-local",
            "--depth",
            str(depth),
            f"file://{repo}",
            str(checkout),
        )
        return checkout

    def merge_no_commit(self, repo, branch):
        """Start a `--no-ff` merge and stop before the commit, so the caller can add to its tree.

        This is the shape that produces the defect: a merge commit whose tree differs from *both*
        parents. `M-34`'s closing was written exactly here — the merge resolved the branch and set
        `status: done` in one commit, which is what "Merge impl/M-34, and close it" means.
        """
        self.git(repo, "merge", "--no-ff", "--no-commit", "-q", branch)

    def test_a_story_closed_inside_a_merge_commit_is_counted(self):
        """`X-55`, the closed half: `git log -p` emits no diff for a merge unless asked.

        `M-34` is the instance. Its `status: done` landed in the merge commit, so the closing appeared
        in no non-merge diff, the history walk never saw it, and the journal came out one ahead of the
        snapshot — which took a hand repair of the generated report to recover from.

        The fixture closes `X-2` *in the merge commit itself* rather than on the branch. Closing it on
        the branch would prove nothing: the default walk visits every parent, so a branch commit's own
        diff is already counted. What is invisible is a change that exists in no parent's diff.
        """
        repo = self.fixture()
        self.git(repo, "switch", "-q", "-c", "impl/X-2")
        self.write_story(repo, "X-9", "ready")
        self.commit(repo, "file X-9 on the branch")
        self.git(repo, "switch", "-q", "-")
        self.merge_no_commit(repo, "impl/X-2")
        self.write_story(repo, "X-2", "done")
        self.commit(repo, "Merge impl/X-2, and close it")

        closed = self.closed_facts(repo)
        self.assertIn(
            "closed:docs/stories/X-2-a-story.md",
            closed,
            "a story closed inside a merge commit must still be counted as closed; `git log -p` "
            "shows no diff for a merge unless asked, so this fact is otherwise lost silently",
        )

    def test_a_story_filed_inside_a_merge_commit_is_counted(self):
        """`X-55`, the filed half: `--diff-filter=A --name-only` has the same default.

        A separate `git log` invocation with the same blind spot, so a story file whose first
        appearance is a merge commit is not counted as filed either. The Acceptance asks for both.
        """
        repo = self.fixture()
        self.git(repo, "switch", "-q", "-c", "impl/X-3")
        self.write_story(repo, "X-9", "ready")
        self.commit(repo, "file X-9 on the branch")
        self.git(repo, "switch", "-q", "-")
        self.merge_no_commit(repo, "impl/X-3")
        self.write_story(repo, "X-3", "ready")
        self.commit(repo, "Merge impl/X-3, and file X-3 while resolving it")

        filed = self.filed_facts(repo)
        self.assertIn(
            "filed:docs/stories/X-3-a-story.md",
            filed,
            "a story file that first appears in a merge commit must be counted as filed",
        )

    def test_a_story_added_on_two_lines_of_history_is_one_filing(self):
        """The over-count the same walk carries, found while deciding `X-55`'s route.

        `S-26` in the real history: `f67ffad` filed it on `main`, and `0236340` on `impl/S-26`
        independently created the same file on a branch cut from an earlier commit. The default walk
        visits both parents, so one filing was counted **twice** — 182 filings against 181 real ones.
        Counting merge diffs without also limiting the walk to the mainline makes this worse rather
        than better: it adds the merge's copy as a third. Restricting the walk to first parents is
        what makes a fact an event on the mainline, counted exactly once wherever it landed.

        **Both sides must file something of their own.** The path limit is the whole `docs/stories`
        directory, so a merge TREESAME with one parent across it has that parent's side pruned by
        git's history simplification and the duplicate is invisible — which is why the first version
        of this test passed against the unfixed script. `S-26`'s merge was TREESAME with neither
        parent, and `X-8`/`X-9` here are what reproduce that.
        """
        repo = self.fixture()
        self.git(repo, "switch", "-q", "-c", "impl/X-3")
        self.write_story(repo, "X-3", "done")
        self.write_story(repo, "X-9", "ready")
        self.commit(repo, "file X-3 already closed, and X-9, on the branch")
        self.git(repo, "switch", "-q", "-")
        self.write_story(repo, "X-3", "ready")
        self.write_story(repo, "X-8", "ready")
        self.commit(repo, "file X-3 as ready, and X-8, on the mainline")
        self.git(repo, "merge", "--no-ff", "-q", "-m", "Merge impl/X-3", "impl/X-3", check=False)
        self.write_story(repo, "X-3", "done")
        self.commit(repo, "Merge impl/X-3")

        filed = self.filed_facts(repo)
        self.assertEqual(
            filed.count("filed:docs/stories/X-3-a-story.md"),
            1,
            "one story filed on two lines of history is one filing, not two",
        )
        for other in ("X-8", "X-9"):
            self.assertEqual(
                filed.count(f"filed:docs/stories/{other}-a-story.md"),
                1,
                f"{other} was filed once on one side of the merge and must still be counted once",
            )

    def facts_in(self, repo):
        """`history_story_fact_days` as the copy of the generator in the fixture computes it.

        Run out of process against the fixture's own `maturity.py` and `cwd`, because `ROOT` is bound
        at import time — the module imported by this test file is rooted in the real repository.
        """
        program = (
            "import importlib.util,json,pathlib,sys\n"
            "spec=importlib.util.spec_from_file_location('m','scripts/maturity.py')\n"
            "m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)\n"
            "filed,closed=m.history_story_fact_days()\n"
            "print(json.dumps({'filed':[i for _,i in filed],'closed':[i for _,i in closed]}))\n"
        )
        done = subprocess.run(
            [sys.executable, "-c", program],
            cwd=repo,
            capture_output=True,
            text=True,
            check=True,
        )
        return json.loads(done.stdout)

    def filed_facts(self, repo):
        return self.facts_in(repo)["filed"]

    def closed_facts(self, repo):
        return self.facts_in(repo)["closed"]

    def test_a_journal_ahead_of_the_snapshot_has_a_documented_repair(self):
        """`X-55`'s last Acceptance item: recovery is a documented command, not a reverse-engineered one.

        This is the state `M-34` left `main` in — "the journal came out one ahead of the snapshot".
        The recorded journal is a floor, so the generator refuses rather than overwriting it, which is
        right and was also a dead end: the only way out was deleting the generated
        `maturity-event-days` line out of `docs/maturity.md` by hand, staging it and regenerating.
        Nothing said so, and a hand-*edited* count fails the basis hash, so the one safe hand edit was
        the one nobody would guess.

        Asserted in both directions. The diagnostic must name the repair, and the repair must actually
        leave the report green — a documented command that does not recover would be worse than none.
        """
        repo = self.fixture(date="2020-01-02T03:04:05+00:00")
        self.replace_journal(repo, {"filed": {"2020-01-02": 3}, "closed": {}})

        refused = self.run_maturity(repo, "--check")
        self.assertNotEqual(refused.returncode, 0, "a journal that disagrees must not be overwritten")
        self.assertIn("journal records 3 filed facts", refused.stderr)
        self.assertIn(
            "--reseed-journal",
            refused.stderr,
            "the diagnostic must name the repair, because regenerating cannot perform it",
        )

        reseeded = self.run_maturity(repo, "--reseed-journal")
        self.assertEqual(reseeded.returncode, 0, reseeded.stderr)
        self.stage(repo, "docs/maturity.md")
        self.assertEqual(
            self.run_maturity(repo, "--check").returncode,
            0,
            "the documented repair must leave the report green without a hand edit",
        )

    def test_reseeding_and_checking_at_once_is_refused(self):
        """`--check` must not be the step that rewrites what it verifies."""
        repo = self.fixture()
        both = self.run_maturity(repo, "--check", "--reseed-journal")
        self.assertNotEqual(both.returncode, 0)
        self.assertIn("run them separately", both.stderr)

    def test_a_shallow_checkout_is_refused_rather_than_miscounted(self):
        """`X-49`: what made `main` and every pull request red, and where it pointed the reader.

        A depth-1 checkout has no history to read filing days out of. `git log` still answers: the
        grafted commit has no parent, so every story file in it reads as *added* there. The filed
        count silently becomes the number of story files that exist.

        That count equalled the real one for as long as every story ever filed still existed. The
        first renumber broke it — `eee4394` refiled `P-6` as `P-7`, which is two filings and one
        surviving file — and the diagnostic accused the event-date journal of recording a fact the
        snapshot did not have. The journal was the one thing that was right.

        The fixture reproduces that shape: three filings across history, two story files at `HEAD`.
        """
        repo = self.fixture()
        (repo / "docs" / "stories" / "X-1-a-story.md").unlink()
        self.write_story(repo, "X-3", "ready")
        self.commit(repo, "renumber X-1 to X-3, which files a story a third time")
        self.assertEqual(self.run_maturity(repo).returncode, 0, "the renumber must regenerate")
        self.commit(repo, "the report of the renumbered board")

        shallow = self.shallow_checkout(repo)
        refused = self.run_maturity(shallow, "--check")
        self.assertNotEqual(refused.returncode, 0, "a truncated history must not answer")
        self.assertIn("shallow checkout", refused.stderr)
        self.assertNotIn("journal", refused.stderr, "the journal is not what is wrong")

        # And the guard is about the depth, not about being a clone: the same checkout with its
        # history filled in is green, which is what `fetch-depth: 0` buys CI.
        self.git(shallow, "fetch", "-q", "--unshallow")
        self.assertEqual(
            self.run_maturity(shallow, "--check").returncode,
            0,
            "an unshallowed checkout reads the same history as the origin",
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
