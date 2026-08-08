#!/usr/bin/env python3
"""Tests for check-report-index.py, the check that keeps the evidence surface reachable.

`docs/coverage.md`, `docs/comparison.md`, `docs/compliance.md` and `docs/maturity.md` are what this
repository offers instead of assertions, and until `X-117` every one of them was reachable only by
already knowing its filename. `X-66` shipped the newest of the four and said so in its own handoff:
consistent, and consistently undiscoverable.

The fix that would fail the same way again is a hand-typed list of four links. It is right on the
day it is written and silent on the day a fifth report lands, which is the same defect one release
later. So the property under test is not *are these four linked* — it is **is the set derived**:

1. **A new report with no entry is a red gate.** The reversed fixture adds a fifth generated report
   to a temporary tree and asserts the check names it. This is the story's Acceptance, and it is the
   one test that would fail if the index were typed by hand.
2. **An entry for a report that no longer exists is a red gate.** Drift runs both ways; a link to a
   deleted page is the same staleness wearing the other hat.
3. **Every entry says what its report deliberately does not.** A reader who opens `coverage.md` to
   learn it is not a quality claim has already paid the cost the index exists to save. The non-claim
   is required to be present, and is required to be carried from the report rather than invented.
4. **The title is read from the report, not typed beside it.** A renamed report renames its own
   link.

The gate registration is asserted here rather than left to `gate.py --check`, for the reason the
sibling suites give: `--check` says the step is accounted for, this says the account is the one the
story asked for. `scripts/build-docs.sh` is the host, because it is already where the unpublished
half of the documentation tree is checked — Docusaurus never sees `docs/`.
"""

import importlib.util
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
CHECKER = ROOT / "scripts" / "check-report-index.py"
BUILD_DOCS = ROOT / "scripts" / "build-docs.sh"


def load_module(name, filename):
    """Import a hyphenated script, which is not a legal module name and so not importable."""
    # `scripts/` holds no package, so a cached `__pycache__` here is untracked litter in a
    # directory that otherwise contains only source.
    sys.dont_write_bytecode = True
    path = ROOT / "scripts" / filename
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


index = load_module("check_report_index", "check-report-index.py")


def flat(text):
    """One line, so an assertion about a sentence is not an assertion about where it wrapped."""
    return " ".join(text.split())


def run(root, *args):
    """The checker as the gate runs it: a process, read by its exit code."""
    return subprocess.run(
        [sys.executable, str(CHECKER), "--root", str(root), *args],
        capture_output=True,
        text=True,
    )


class tree:
    """A miniature repository holding only what the checker reads, so a fixture can mutate it.

    Copied rather than synthesized: the reports' own first lines are the input under test, and a
    fixture that writes its own version of them would be testing the fixture.
    """

    def __enter__(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.directory.name)
        (self.root / "docs").mkdir()
        for path in sorted((ROOT / "docs").glob("*.md")):
            shutil.copy(path, self.root / "docs" / path.name)
        (self.root / "scripts").mkdir()
        for report in index.discover(ROOT):
            (self.root / report.generator).write_text("")
            if report.published is not None:
                page = self.root / report.published
                page.parent.mkdir(parents=True, exist_ok=True)
                page.write_text(f"<!-- BEGIN generated:{report.path.stem} -->\n")
        return self

    def __exit__(self, *_):
        self.directory.cleanup()

    def write(self, relative, text):
        (self.root / relative).write_text(text)

    def read(self, relative):
        return (self.root / relative).read_text()


class Discovery(unittest.TestCase):
    """What counts as a generated report is read off the tree, not listed."""

    def test_the_four_reports_are_found(self):
        found = {report.path.name for report in index.discover(ROOT)}
        self.assertEqual(
            found, {"coverage.md", "comparison.md", "compliance.md", "maturity.md"}
        )

    def test_a_hand_written_page_is_not_a_report(self):
        found = {report.path.name for report in index.discover(ROOT)}
        for page in ("vision.md", "roadmap.md", "rfc-roadmap.md", "README.md"):
            self.assertNotIn(page, found)

    def test_each_report_names_a_generator_that_exists(self):
        for report in index.discover(ROOT):
            self.assertTrue((ROOT / report.generator).exists(), report.generator)

    def test_the_title_is_read_from_the_report(self):
        titles = {report.path.name: report.title for report in index.discover(ROOT)}
        self.assertEqual(titles["compliance.md"], "RFC compliance")
        # Renaming the report's own heading renames its link, with nothing typed twice.
        with tree() as fixture:
            fixture.write(
                "docs/compliance.md",
                fixture.read("docs/compliance.md").replace(
                    "# RFC compliance", "# RFC compliance, per role", 1
                ),
            )
            rendered = flat(index.render(index.discover(fixture.root)))
            self.assertIn("[RFC compliance, per role](compliance.md)", rendered)


class Reachability(unittest.TestCase):
    """The reversed fixtures: every way the index can stop describing the tree."""

    def test_the_repository_index_is_current(self):
        result = run(ROOT, "--check")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_a_new_generated_report_with_no_entry_fails(self):
        with tree() as fixture:
            fixture.write(
                "docs/telemetry.md",
                "# Telemetry: what the endpoint emits\n\n"
                "<!-- Generated by scripts/maturity.py from nothing. Do not edit. -->\n",
            )
            result = run(fixture.root, "--check")
            self.assertEqual(result.returncode, 1)
            self.assertIn("telemetry.md", result.stderr)

    def test_a_new_generated_report_cannot_be_made_reachable_by_regenerating_alone(self):
        # The writer is not an escape hatch: a report nobody has described has nothing to write.
        with tree() as fixture:
            fixture.write(
                "docs/telemetry.md",
                "# Telemetry: what the endpoint emits\n\n"
                "<!-- Generated by scripts/maturity.py from nothing. Do not edit. -->\n",
            )
            result = run(fixture.root)
            self.assertEqual(result.returncode, 1)
            self.assertIn("telemetry.md", result.stderr)

    def test_an_entry_for_a_report_that_no_longer_exists_fails(self):
        with tree() as fixture:
            (fixture.root / "docs" / "coverage.md").unlink()
            result = run(fixture.root, "--check")
            self.assertEqual(result.returncode, 1)
            self.assertIn("coverage.md", result.stderr)

    def test_an_unlinked_report_fails(self):
        with tree() as fixture:
            text = fixture.read("docs/README.md")
            before, _, rest = text.partition(index.BEGIN)
            _, _, after = rest.partition(index.END)
            fixture.write(
                "docs/README.md", before + index.BEGIN + "\n" + index.END + after
            )
            result = run(fixture.root, "--check")
            self.assertEqual(result.returncode, 1)
            self.assertIn("out of date", result.stderr)

    def test_an_index_with_no_region_fails(self):
        with tree() as fixture:
            fixture.write("docs/README.md", "# sipx docs\n")
            result = run(fixture.root, "--check")
            self.assertEqual(result.returncode, 1)
            self.assertIn(index.BEGIN, result.stderr)

    def test_a_report_naming_a_generator_that_does_not_exist_fails(self):
        with tree() as fixture:
            (fixture.root / "scripts" / "coverage-report.py").unlink()
            result = run(fixture.root, "--check")
            self.assertEqual(result.returncode, 1)
            self.assertIn("coverage-report.py", result.stderr)

    def test_regenerating_makes_the_check_green(self):
        with tree() as fixture:
            fixture.write("docs/README.md", index.BEGIN + "\n" + index.END + "\n")
            self.assertEqual(run(fixture.root, "--check").returncode, 1)
            self.assertEqual(run(fixture.root).returncode, 0)
            self.assertEqual(run(fixture.root, "--check").returncode, 0)


class Entries(unittest.TestCase):
    """What each link has to say, so opening the page is not how a reader learns its limits."""

    def test_every_report_states_what_it_deliberately_does_not(self):
        for report in index.discover(ROOT):
            entry = index.ENTRIES[report.path.name]
            self.assertTrue(entry.measures.strip(), report.path.name)
            self.assertTrue(entry.deliberately_not.strip(), report.path.name)

    def test_the_non_claims_are_carried_from_the_reports(self):
        """Each non-claim is the report's own, not a weaker paraphrase invented here.

        Every phrase below is quoted from the page it describes, so the assertion fails if an
        entry is ever softened into something the report itself does not say.
        """
        rendered = flat(index.render(index.discover(ROOT)))
        for source, phrase in (
            # docs/coverage.md, "What this is not".
            ("docs/coverage.md", "threshold gates the build on any number"),
            ("docs/coverage.md", "not whether executing it proved anything"),
            # docs/comparison.md, on its own asymmetry.
            ("docs/comparison.md", "somebody reading someone else's code"),
            # docs/compliance.md, on what the registry check cannot do.
            ("docs/compliance.md", "cannot check that behaviour is *correct*"),
            # docs/maturity.md, "What this cannot see".
            ("docs/maturity.md", "whether the tests are good"),
        ):
            self.assertIn(phrase, rendered)
            self.assertIn(phrase, flat((ROOT / source).read_text()), source)

    def test_the_rendered_region_links_every_report(self):
        rendered = index.render(index.discover(ROOT))
        for report in index.discover(ROOT):
            self.assertIn(f"]({report.path.name})", rendered)

    def test_a_published_report_says_where_it_is_published(self):
        published = {
            report.path.name: report.published for report in index.discover(ROOT)
        }
        self.assertEqual(
            published["comparison.md"],
            pathlib.PurePath("website/docs/reference/comparison.md"),
        )
        self.assertEqual(
            published["compliance.md"],
            pathlib.PurePath("website/docs/reference/compliance.md"),
        )
        # Internal by design: neither is a page a user of the library is asked to read.
        self.assertIsNone(published["coverage.md"])
        self.assertIsNone(published["maturity.md"])


class Registration(unittest.TestCase):
    """A check nothing runs is a comment."""

    def test_the_docs_step_runs_the_check_and_this_suite(self):
        script = BUILD_DOCS.read_text()
        self.assertIn("./scripts/check-report-index.py --check", script)
        self.assertIn("python3 scripts/test-report-index.py", script)

    def test_the_step_states_what_it_checks(self):
        """`build-docs.sh`'s header enumerates its contract; a check missing from it is invisible."""
        self.assertIn("check-report-index.py --check", BUILD_DOCS.read_text().split("set -euo")[0])

    def test_the_check_runs_before_its_own_suite(self):
        """An unreachable report fails both. The one that names the page has to speak first.

        Reversed, the reader gets a unittest traceback for
        `test_the_repository_index_is_current` instead of the sentence naming the page and the
        entry it needs — a docs defect surfacing as a generic docs-site failure.
        """
        script = BUILD_DOCS.read_text()
        self.assertLess(
            script.index("./scripts/check-report-index.py --check"),
            script.index("python3 scripts/test-report-index.py"),
        )


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
