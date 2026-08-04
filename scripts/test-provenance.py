#!/usr/bin/env python3
"""Tests for scripts/check-provenance.sh.

The provenance gate had no suite until `X-71` gave it an exception. An exception is exactly
the thing that rots: `COMPARISON_SCOPE` is three pathspecs, and a fourth added in a hurry, or
a pathspec that stops being passed to `git grep`, turns the gate into a check that reports
clean because it looked nowhere. That is the `X-36` shape — it looks like coverage and is not.

Every test builds a throwaway git repository and runs the real script against it with a
denylist of its own, so nothing here depends on the private denylist being present. The term
used is deliberately nonsense: a real denylisted term written into this file would be caught
by the very check under test, which is a funny failure to debug once and a waste of an hour.
"""

import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
CHECK = ROOT / "scripts" / "check-provenance.sh"

# Not a real denylisted term. See the module docstring.
TERM = "zzfictionalpriorart"

IN_SCOPE = (
    "docs/comparison/observations/subject.json",
    "docs/comparison.md",
    "website/docs/reference/comparison.md",
)


def git(repo, *args):
    """Run git in `repo`, failing loudly — a test that cannot set up its fixture is not a pass."""
    subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
    )


def a_repository(tmp):
    """A git repository holding the real script, one clean tracked file, and nothing else."""
    repo = pathlib.Path(tmp)
    (repo / "scripts").mkdir(parents=True)
    shutil.copy2(CHECK, repo / "scripts" / "check-provenance.sh")
    (repo / "README.md").write_text("A repository with nothing to hide.\n")

    git(repo, "init", "-q")
    git(repo, "config", "user.email", "test@example.invalid")
    git(repo, "config", "user.name", "Test")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "initial")
    return repo


def write(repo, relative, text):
    """Create a tracked file — `git grep` sees tracked files only, so untracked proves nothing."""
    path = repo / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    git(repo, "add", str(path))


def check(repo, *args, denylist=TERM, ci=False):
    env = dict(os.environ)
    env.pop("SIPX_DENYLIST", None)
    env.pop("SIPX_DENYLIST_FILE", None)
    env.pop("CI", None)
    if denylist is not None:
        env["SIPX_DENYLIST"] = denylist
    if ci:
        env["CI"] = "1"
    # HOME is redirected so the local ~/notes fallback cannot rescue a test that meant to run
    # with no denylist configured.
    env["HOME"] = str(repo / "nonexistent-home")
    return subprocess.run(
        [str(repo / "scripts" / "check-provenance.sh"), *args],
        capture_output=True,
        text=True,
        env=env,
    )


class TheComparisonScope(unittest.TestCase):
    """X-71: a subject may be named in the comparison artifacts, and nowhere else."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = a_repository(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)

    def test_a_clean_repository_passes(self):
        result = check(self.repo)
        self.assertEqual(0, result.returncode, result.stderr)

    def test_each_in_scope_path_may_name_a_subject(self):
        for relative in IN_SCOPE:
            with self.subTest(path=relative):
                tmp = tempfile.TemporaryDirectory()
                self.addCleanup(tmp.cleanup)
                repo = a_repository(tmp.name)
                write(repo, relative, f"comparing against {TERM} at v1.2.3\n")
                result = check(repo)
                self.assertEqual(
                    0,
                    result.returncode,
                    f"{relative} is inside COMPARISON_SCOPE and was rejected; {result.stderr}",
                )

    def test_a_spec_may_not_name_a_subject(self):
        write(self.repo, "docs/specs/transport.md", f"we do it the way {TERM} does\n")
        result = check(self.repo)
        self.assertEqual(1, result.returncode, "a spec named a subject and the gate passed")
        self.assertIn("docs/specs/transport.md", result.stderr)

    def test_source_may_not_name_a_subject(self):
        write(self.repo, "crates/sipx-sip/src/parser.rs", f"// modelled on {TERM}\n")
        result = check(self.repo)
        self.assertEqual(1, result.returncode, "source named a subject and the gate passed")

    def test_a_path_merely_containing_the_word_comparison_is_not_in_scope(self):
        """The scope is three paths, not any file whose name suggests a comparison."""
        write(self.repo, "docs/designs/comparison-notes.md", f"{TERM} does it differently\n")
        result = check(self.repo)
        self.assertEqual(
            1,
            result.returncode,
            "a design doc rode the exception on the strength of its filename",
        )

    def test_an_in_scope_file_does_not_excuse_an_out_of_scope_one(self):
        write(self.repo, "docs/comparison.md", f"{TERM} v1\n")
        write(self.repo, "docs/vision.md", f"informed by {TERM}\n")
        result = check(self.repo)
        self.assertEqual(1, result.returncode)
        self.assertIn("docs/vision.md", result.stderr)
        self.assertNotIn("docs/comparison.md", result.stderr)


class TheHistoryScan(unittest.TestCase):
    """The exception is for file contents. Commit messages have none, deliberately."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = a_repository(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)

    def test_a_commit_message_naming_a_subject_fails_even_when_every_file_is_in_scope(self):
        write(self.repo, "docs/comparison.md", f"{TERM} v1.2.3\n")
        git(self.repo, "commit", "-q", "-m", f"compare against {TERM}")
        result = check(self.repo, "--history")
        self.assertEqual(
            1,
            result.returncode,
            "a subject name landed in a commit message and --history passed",
        )
        self.assertIn("History must be rewritten", result.stderr)

    def test_an_in_scope_file_alone_leaves_history_clean(self):
        write(self.repo, "docs/comparison.md", f"{TERM} v1.2.3\n")
        git(self.repo, "commit", "-q", "-m", "record the comparison dataset")
        result = check(self.repo, "--history")
        self.assertEqual(0, result.returncode, result.stderr)


class TheDenylistConfiguration(unittest.TestCase):
    """An unconfigured gate that passes is worse than no gate — but only CI can insist."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = a_repository(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)

    def test_no_denylist_in_ci_is_a_misconfiguration(self):
        result = check(self.repo, denylist=None, ci=True)
        self.assertEqual(2, result.returncode, result.stdout + result.stderr)

    def test_no_denylist_locally_skips_rather_than_failing(self):
        result = check(self.repo, denylist=None)
        self.assertEqual(0, result.returncode, result.stderr)
        self.assertIn("skipped", result.stdout)


class TheScriptItself(unittest.TestCase):
    """Structural guarantees that a behavioural test would only catch by accident."""

    def setUp(self):
        self.text = CHECK.read_text()

    def test_the_scope_is_a_named_constant(self):
        self.assertIn(
            "COMPARISON_SCOPE=(",
            self.text,
            "the scope stopped being a reviewable constant",
        )

    def test_the_scope_lists_exactly_the_comparison_artifacts(self):
        block = self.text.split("COMPARISON_SCOPE=(", 1)[1].split(")", 1)[0]
        listed = {line.strip().strip("'\"") for line in block.splitlines() if line.strip()}
        self.assertEqual(
            {
                ":!docs/comparison",
                ":!docs/comparison.md",
                ":!website/docs/reference/comparison.md",
            },
            listed,
            "COMPARISON_SCOPE changed; widening it is a decision that needs a story",
        )

    def test_the_history_scan_is_not_subject_to_the_scope(self):
        """The one failure with no cheap remedy stays unreachable."""
        history = self.text.split("scan_history -eq 1", 1)[1]
        self.assertNotIn(
            "COMPARISON_SCOPE",
            history,
            "the file-scan exception leaked into the commit-message scan",
        )


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
