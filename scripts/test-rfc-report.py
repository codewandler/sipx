#!/usr/bin/env python3
"""Tests for rfc-report.py's schema guard.

The registry's grain is per RFC, decided in `docs/designs/rfc-registry-grain.md`. A decision
that lives only in prose is a convention, and conventions rot: the way this one rots is that
somebody adds a finer-grained row, `tomllib` parses it happily, and the checker walks straight
past it — so the claim is in the source, absent from the generated table, and nobody is told.

These tests hold the guard that makes the decision enforceable rather than merely stated.
"""

import importlib.util
import pathlib
import sys
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent


def load_report_module():
    """Import rfc-report.py, whose hyphen keeps it out of the normal import path."""
    # `scripts/` holds no package, so a cached `__pycache__` here is untracked litter in a
    # directory that otherwise contains only source.
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location(
        "rfc_report", ROOT / "scripts" / "rfc-report.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


report = load_report_module()


def registry_entries():
    return tomllib.loads((ROOT / "docs" / "rfc" / "registry.toml").read_text())["rfc"]


def an_entry(**overrides):
    """A minimal well-formed entry, so a test can vary exactly one thing about it."""
    entry = {
        "number": 9999,
        "title": "A tracked document",
        "layer": "wire",
        "status": "none",
        "evidence": [],
        "note": "",
    }
    entry.update(overrides)
    return entry


class SchemaGuard(unittest.TestCase):
    """An entry may carry the keys the schema names, and no others."""

    def test_requirement_grain_row_is_rejected(self):
        """`[[rfc.requirement]]` must fail loudly, not be silently dropped.

        This is the decision under test. X-15 declined requirement-grain rows; the failure mode
        that decision has to survive is a row added anyway — by a contributor, or by a downstream
        registry merging its own extension back. `tomllib` turns `[[rfc.requirement]]` into a
        `requirement` key on the entry, so the guard sees it as an unknown key.
        """
        entry = an_entry(requirement=[{"section": "4.4.2", "status": "implemented"}])
        problems = report.schema_problems(entry)
        self.assertTrue(
            any("requirement" in p for p in problems),
            f"a requirement-grain row was accepted in silence; problems={problems}",
        )

    def test_requirement_row_is_rejected_through_the_public_check(self):
        """The guard has to fire from `--check`, which is the only entry point CI runs."""
        entries = registry_entries()
        entries.append(an_entry(number=9999, requirement=[{"section": "1"}]))
        problems = report.check(entries)
        self.assertTrue(
            any("requirement" in p and "9999" in p for p in problems),
            f"check() did not report the requirement row; problems={problems}",
        )

    def test_misspelled_key_is_rejected(self):
        """`role` is not `roles`, and the Roles column is load-bearing.

        The generated table renders a missing `roles` as an em dash. A typo therefore understates
        which roles sipx implements — the one direction of error the table must not make silently,
        since the whole point of the column is that a UA implementation does not imply the proxy
        half.
        """
        problems = report.schema_problems(an_entry(role=["uac"]))
        self.assertTrue(
            any("role" in p for p in problems),
            f"a misspelled key was accepted; problems={problems}",
        )

    def test_missing_required_key_is_rejected(self):
        entry = an_entry()
        del entry["note"]
        problems = report.schema_problems(entry)
        self.assertTrue(
            any("note" in p for p in problems),
            f"a missing required key was accepted; problems={problems}",
        )

    def test_a_well_formed_entry_is_accepted(self):
        """The guard must not cost a legitimate entry anything."""
        entry = an_entry(
            status="implemented",
            evidence=["scripts/rfc-report.py"],
            roles=["uac", "uas"],
            headers=["Via"],
            methods=["INVITE"],
            note="A note.",
        )
        self.assertEqual([], report.schema_problems(entry))


class TheRealRegistry(unittest.TestCase):
    """The guard is only worth having if the registry it guards already satisfies it."""

    def test_every_entry_satisfies_the_schema(self):
        for entry in registry_entries():
            with self.subTest(rfc=entry.get("number")):
                self.assertEqual([], report.schema_problems(entry))

    def test_the_registry_has_no_outstanding_problems(self):
        self.assertEqual([], report.check(registry_entries()))

    def test_rfc_number_is_a_usable_reference_key(self):
        """A downstream inherits a kernel row by its RFC number, so the number must identify one.

        `docs/rfc/README.md` promises exactly this to a downstream registry pinning a kernel
        version. Uniqueness is what makes `inherits = 3261` resolve to a single claim.
        """
        numbers = [e["number"] for e in registry_entries()]
        self.assertEqual(len(numbers), len(set(numbers)))
        for number in numbers:
            self.assertIsInstance(number, int)


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
