#!/usr/bin/env python3
"""Tests for `contention-proof.py`, whose whole product is one distinction.

The script answers "did the bounded assertions hold on a busy machine". The only interesting way
for it to be wrong is to answer *proven* when nothing was measured — a control that could not fail,
a subject list that matched no test. Both are silent, both look exactly like success, and both are
the shape this repository has shipped before. So that is what these tests are about.

Nothing here starts a burner or runs cargo. The verdict is a pure function of two facts and is
tested as one; the name resolution reads the real source files, which is the point of it.
"""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location(
    "contention_proof", ROOT / "scripts" / "contention-proof.py"
)
proof = importlib.util.module_from_spec(_spec)
# Registered before execution because the module defines a `@dataclass`, and `dataclasses` resolves
# a field's annotation through `sys.modules[cls.__module__]`. Executing an unregistered module
# leaves that lookup holding `None`.
sys.modules[_spec.name] = proof
_spec.loader.exec_module(proof)


class TheControlIsReadFirst(unittest.TestCase):
    """A run whose control did not fail is inconclusive, whatever the subjects did."""

    def test_a_control_that_passed_makes_green_subjects_inconclusive(self):
        answer, why = proof.verdict({"a": True, "b": True}, control_failed=False)
        self.assertEqual(answer, proof.INCONCLUSIVE)
        self.assertIn(proof.CONTROL.name, why)

    def test_a_control_that_passed_is_inconclusive_even_with_red_subjects(self):
        """Not `failed`: with the harness unproven, a red subject is not evidence either."""
        answer, _ = proof.verdict({"a": False}, control_failed=False)
        self.assertEqual(answer, proof.INCONCLUSIVE)

    def test_no_subjects_is_inconclusive_rather_than_proven(self):
        """An empty measurement is the classic false green. `all([])` is `True`; this is not."""
        answer, _ = proof.verdict({}, control_failed=True)
        self.assertEqual(answer, proof.INCONCLUSIVE)


class TheVerdictNamesWhatWentRed(unittest.TestCase):
    def test_a_red_subject_beside_a_red_control_is_a_failure(self):
        answer, why = proof.verdict({"a": True, "b": False}, control_failed=True)
        self.assertEqual(answer, proof.FAILED)
        self.assertIn("b", why)
        self.assertNotIn("a,", why)

    def test_green_subjects_beside_a_red_control_is_the_proof(self):
        answer, why = proof.verdict({"a": True, "b": True}, control_failed=True)
        self.assertEqual(answer, proof.PROVEN)
        self.assertIn("2", why)


class TheNamesResolveAgainstTheRealSource(unittest.TestCase):
    """The way this proof rots: a test is renamed, `--exact` matches nothing, cargo exits 0."""

    def test_the_tree_as_it_stands_resolves(self):
        self.assertEqual(proof.check_problems(), [])

    def test_a_missing_test_is_reported(self):
        renamed = proof.Subject(
            "--test", "cli", "a_test_nobody_wrote", "crates/sipx-cli/tests/cli.rs"
        )
        with _subjects((renamed,)):
            problems = proof.check_problems()
        self.assertTrue(any("a_test_nobody_wrote" in problem for problem in problems), problems)
        self.assertTrue(any("proof of nothing" in problem for problem in problems), problems)

    def test_a_missing_source_file_is_reported(self):
        moved = proof.Subject("--test", "cli", "whatever", "crates/sipx-cli/tests/gone.rs")
        with _subjects((moved,)):
            problems = proof.check_problems()
        self.assertTrue(any("does not exist" in problem for problem in problems), problems)


class TheControlStaysAControl(unittest.TestCase):
    def test_the_control_is_ignored_and_unbounded_in_the_tree(self):
        source = (ROOT / proof.CONTROL.source).read_text(encoding="utf-8")
        head, _, body = source.partition(f"fn {proof.CONTROL.name}")
        self.assertIn("#[ignore", head[-600:], "the control must stay out of ordinary runs")
        self.assertIn("pending", body[:600], "the control must have no way to pass")


class TheCargoInvocationRunsExactlyOneTest(unittest.TestCase):
    def test_exact_is_passed_so_a_prefix_cannot_widen_the_run(self):
        command = proof.SUBJECTS[0].command()
        self.assertIn("--exact", command)
        self.assertEqual(command[-1], proof.SUBJECTS[0].name)
        self.assertNotIn("--ignored", command)

    def test_the_control_is_run_with_ignored(self):
        self.assertEqual(proof.CONTROL.command(ignored=True)[-1], "--ignored")


class _subjects:
    """Swap `SUBJECTS` for the duration of a block, and put it back."""

    def __init__(self, replacement):
        self.replacement = replacement

    def __enter__(self):
        self.original = proof.SUBJECTS
        proof.SUBJECTS = self.replacement
        return self

    def __exit__(self, *_):
        proof.SUBJECTS = self.original
        return False


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
