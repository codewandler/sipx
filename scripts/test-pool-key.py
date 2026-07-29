#!/usr/bin/env python3
"""Tests for check-pool-key.py, the guard that holds the specs to `ConnectionKey`.

The guard replaces a sentence that was wrong twice, so the thing worth testing is that it would
have caught both times: a field added to the key and not to the spec, and a spec that talks
about the key while pointing at nothing. Both are silent failures — the shape of defect where a
check that quietly matches nothing is indistinguishable from a check that passes.

The false-positive direction is tested too. `docs/specs` also pools isolates and refuses a
shared pool of applications, and a guard that fired on those would be switched off by the second
person who hit it.
"""

import importlib.util
import pathlib
import re
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent


def load_module():
    """Import check-pool-key.py, whose hyphen keeps it out of the normal import path."""
    # `scripts/` holds no package, so a cached `__pycache__` here is untracked litter in a
    # directory that otherwise contains only source.
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location(
        "check_pool_key", ROOT / "scripts" / "check-pool-key.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


guard = load_module()


def source_text() -> str:
    return (ROOT / guard.SOURCE).read_text(encoding="utf-8")


class TheRepositoryItself(unittest.TestCase):
    """The state the gate demands, asserted here so a failure names which half broke."""

    def test_the_spec_matches_the_type(self):
        self.assertEqual([], guard.region_problems(update=False))

    def test_every_other_spec_cites_the_definition(self):
        self.assertEqual([], guard.citation_problems())


class TheStructReader(unittest.TestCase):
    """Everything downstream is derived from this, including what the spec claims."""

    def setUp(self):
        self.fields = guard.fields(source_text())

    def test_it_reads_the_key_the_type_declares(self):
        self.assertEqual(
            ["peer", "transport", "identity", "path"], [f.name for f in self.fields]
        )

    def test_every_field_brings_its_documentation(self):
        """The doc comment *is* the spec's description; a field with none renders a blank cell."""
        for field in self.fields:
            with self.subTest(field=field.name):
                self.assertTrue(field.doc.strip(), f"`{field.name}` has no doc comment")

    def test_a_source_it_cannot_read_is_an_error_not_an_empty_result(self):
        with self.assertRaises(ValueError):
            guard.fields("pub struct SomethingElse {\n    pub peer: SocketAddr,\n}\n")

    def test_an_implausibly_small_key_is_an_error(self):
        """A reader that has drifted finds one field and renders a table nobody would question."""
        with self.assertRaises(ValueError):
            guard.fields(
                f"pub struct {guard.TYPE} {{\n    /// The far end.\n    pub peer: SocketAddr,\n}}\n"
            )

    def test_multi_line_documentation_becomes_one_cell(self):
        found = guard.fields(
            f"pub struct {guard.TYPE} {{\n"
            "    /// The far end,\n    /// as the socket sees it.\n    pub peer: SocketAddr,\n"
            "    /// Which transport it speaks.\n    pub transport: TransportKind,\n"
            "}\n"
        )
        self.assertEqual("The far end, as the socket sees it.", found[0].doc)


class TheGeneratedRegion(unittest.TestCase):
    """What the spec shows, and that it moves when the type moves."""

    def setUp(self):
        self.fields = guard.fields(source_text())
        self.spec = guard.DEFINITION.read_text(encoding="utf-8")

    def test_the_spec_shows_every_field_and_its_documentation(self):
        region = guard.REGION.search(self.spec).group(0)
        for field in self.fields:
            with self.subTest(field=field.name):
                self.assertIn(f"`{field.name}`", region)
                self.assertIn(field.doc, region)

    def test_a_field_added_to_the_key_no_longer_matches_the_spec(self):
        """The defect's own shape: the key grows and the prose does not.

        It happened when the verified identity joined the key and again when the WebSocket
        resource did, and both times nothing said so.
        """
        grown = list(self.fields) + [guard.Field("cipher", "The suite that was negotiated.")]
        self.assertNotIn(guard.render(grown), self.spec)

    def test_a_field_removed_from_the_key_no_longer_matches_the_spec(self):
        shrunk = [f for f in self.fields if f.name != "path"]
        self.assertNotIn(guard.render(shrunk), self.spec)

    def test_a_pipe_in_a_doc_comment_does_not_break_the_table(self):
        """A cell separator inside a description would silently split the row into three."""
        rendered = guard.render(
            [guard.Field("peer", "Either | or the other."), guard.Field("transport", "How.")]
        )
        row = [line for line in rendered.splitlines() if line.startswith("| `peer`")][0]
        cells = [cell for cell in re.split(r"(?<!\\)\|", row) if cell.strip()]
        self.assertEqual(2, len(cells), f"the row split into extra cells: {row}")


class TheCitationRule(unittest.TestCase):
    """A spec that speaks of the key without naming where it is defined is how this started."""

    def test_sections_are_split_at_their_headings(self):
        found = guard.sections("intro\n\n## 1. One\n\nbody one\n\n## 2. Two\n\nbody two\n")
        self.assertEqual(["(preamble)", "1. One", "2. Two"], [h for h, _ in found])
        self.assertIn("body one", dict(found)["1. One"])
        self.assertNotIn("body two", dict(found)["1. One"])

    def test_the_narrow_phrases_are_what_the_specs_actually_say(self):
        for phrase in ("The pool keys connections by", "Connections are pooled by", "the pool key"):
            with self.subTest(phrase=phrase):
                self.assertTrue(guard.SPEAKS_OF_THE_KEY.search(phrase))

    def test_an_unrelated_pool_is_not_the_connection_pool(self):
        """`docs/specs` pools isolates and refuses a shared pool of apps. Neither is this key."""
        for phrase in (
            "Isolate granularity and pooling; handler load/reload lifecycle",
            "There is no shared pool, no app inheritance",
            "the pool entry is unchanged",
        ):
            with self.subTest(phrase=phrase):
                self.assertIsNone(guard.SPEAKS_OF_THE_KEY.search(phrase))


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
