#!/usr/bin/env python3
"""Tests for the executable CLI-reference drift check."""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "check-cli-reference.py"
SPEC = importlib.util.spec_from_file_location("check_cli_reference", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
checker = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = checker
SPEC.loader.exec_module(checker)


ROOT_HELP = """\
COMMANDS:
    dial        Place a call
    help        Show this message
    version     Show the version

GLOBAL OPTIONS:
    --json      JSON
"""

DIAL_HELP = """\
OPTIONS:
    --timeout <S>  Bound setup
    --json         JSON
    -h, --help     Help
"""

DOCUMENT = """\
# CLI reference

## `sipx dial <URI>`

| Flag | Meaning |
|---|---|
| `--timeout <S>` | Bound setup |

## The JSON contract

<!-- BEGIN cli-json-contracts -->
| Contract | Producer | Required structural fields |
|---|---|---|
| `sipx.test.v1` | `dial` | `schema`, `status` |
<!-- END cli-json-contracts -->
"""


class HelpComparison(unittest.TestCase):
    def test_an_undocumented_executable_flag_is_a_failure(self):
        help_with_new_flag = DIAL_HELP + "    --new-flag <X>  New\n"
        problems = checker.help_drift(DOCUMENT, ROOT_HELP, {"dial": help_with_new_flag})
        self.assertTrue(any("--new-flag" in problem for problem in problems))

    def test_a_documented_flag_absent_from_help_is_a_failure(self):
        document = DOCUMENT.replace(
            "| `--timeout <S>` | Bound setup |",
            "| `--timeout <S>` | Bound setup |\n| `--vanished` | Gone |",
        )
        problems = checker.help_drift(document, ROOT_HELP, {"dial": DIAL_HELP})
        self.assertTrue(any("--vanished" in problem for problem in problems))

    def test_help_and_document_agree_without_repeating_global_flags(self):
        self.assertEqual(checker.help_drift(DOCUMENT, ROOT_HELP, {"dial": DIAL_HELP}), [])


class JsonComparison(unittest.TestCase):
    def test_a_rust_module_name_may_identify_the_producer(self):
        document = DOCUMENT.replace("`dial`", "`load_responder`")
        actual = {
            "sipx.test.v1": checker.JsonContract("load_responder", {"schema", "status"})
        }
        self.assertEqual(checker.json_drift(document, actual), [])

    def test_a_missing_json_field_is_a_failure(self):
        actual = {"sipx.test.v1": checker.JsonContract("dial", {"schema", "status", "peer"})}
        problems = checker.json_drift(DOCUMENT, actual)
        self.assertTrue(any("peer" in problem for problem in problems))

    def test_a_new_versioned_contract_is_a_failure(self):
        actual = {
            "sipx.test.v1": checker.JsonContract("dial", {"schema", "status"}),
            "sipx.new.v1": checker.JsonContract("dial", {"schema"}),
        }
        problems = checker.json_drift(DOCUMENT, actual)
        self.assertTrue(any("sipx.new.v1" in problem for problem in problems))

    def test_a_versioned_producer_without_strict_output_coverage_is_a_failure(self):
        actual = {"sipx.test.v1": checker.JsonContract("dial", {"schema"})}
        problems = checker.strict_output_drift(actual, set())
        self.assertTrue(any("dial" in problem for problem in problems))

    def test_current_versioned_producers_have_strict_process_coverage(self):
        actual = checker.discover_json_contracts(ROOT)
        covered = checker.strict_result_producers(
            checker.STRICT_JSON_TEST.read_text(encoding="utf-8")
        )
        self.assertEqual(checker.strict_output_drift(actual, covered), [])

    def test_current_source_inventory_matches_the_public_table(self):
        document = checker.DOCUMENT.read_text(encoding="utf-8")
        self.assertEqual(checker.json_drift(document, checker.discover_json_contracts(ROOT)), [])


class TheReferenceBuild(unittest.TestCase):
    """The reference build must not land on the binary the process tests spawn.

    `gate.py` runs this check after the all-features `test` step, so building into the shared
    directory left every gate run ending with a default-feature `sipx`. The next run's process
    tests spawned it and failed as though audio were broken.
    """

    def _build_source(self) -> str:
        source = pathlib.Path(SCRIPT).read_text(encoding="utf-8")
        start = source.index("def build_binary(")
        return source[start : source.index("\ndef ", start)]

    def test_the_reference_build_uses_its_own_target_directory(self) -> None:
        build = self._build_source()
        self.assertIn(
            "--target-dir",
            build,
            "build_binary writes into the shared target directory, so a gate run ends by leaving "
            "a default-feature sipx for the next run's process tests to spawn",
        )
        self.assertIn("cli-reference", build)

    def test_the_reference_build_stays_default_feature(self) -> None:
        build = self._build_source()
        self.assertNotIn(
            "--all-features",
            build,
            "the reference documents the default-feature surface a reader installing sipx-cli "
            "gets; building it with every feature would document a different binary",
        )


class TheOptionReader(unittest.TestCase):
    """`X-131`: a command's options are the entries of its help, not every `--token` in the text.

    The reader used to match `--[a-z][a-z0-9-]*` over the whole of a command's `--help`, so a
    sentence that named another command's flag became a flag of the command that named it. Writing
    "give it headroom over the generator's `--concurrency`" on `load-responder`'s `--max-active`
    reported ``load-responder: executable option `--concurrency` is not documented``, and the
    sentence had to be reworded around the checker.

    Narrowing a reader is how one goes quiet, so both directions are asserted below and in both of
    clap's layouts: the long help's one-entry-per-line block, which is what the binary renders and
    where the encounter happened, and the compact two-column block. The third row is the property
    that makes over-narrowing safe to attempt at all — `help_drift` compares both ways, so a reader
    that stopped seeing options would be reported by the page half of the same comparison rather
    than passing silently.
    """

    #: A root help naming the one command these fixtures describe.
    ROOT = "Commands:\n  load-responder  Answer a load\n  help  Show this message\n"

    #: The long-help layout: the entry opens the line, everything about it is indented under it.
    HELP = """\
Options:
      --json
          Report command results as JSON on stdout

      --max-active <MAX_ACTIVE>
          Ceiling on simultaneously owned dialogs. Give it headroom over the generator's
          `--concurrency` rather than matching it

  -v...
          Show load progress

  -h, --help
          Print help
"""

    #: The compact layout, where the description shares the entry's line.
    COMPACT = """\
OPTIONS:
    --max-active <N>  Ceiling on dialogs; give it headroom over the generator's `--concurrency`
    --json            JSON
    -h, --help        Help
"""

    DOCUMENT = """\
# CLI reference

## `sipx load-responder`

| Flag | Meaning |
|---|---|
| `--max-active <N>` | Ceiling on dialogs |
"""

    def test_a_flag_named_in_prose_is_not_an_option_of_the_command_that_named_it(self):
        self.assertEqual([], self.drift(self.HELP))

    def test_the_same_holds_where_the_description_shares_the_entry_s_line(self):
        self.assertEqual([], self.drift(self.COMPACT))

    def test_an_undocumented_option_is_still_a_failure(self):
        """The case the wide reader existed for: a real entry the public page does not carry."""
        problems = self.drift(self.HELP + "\n      --cleanup <S>\n          Drain deadline\n")
        self.assertEqual(1, len(problems))
        self.assertIn("--cleanup", problems[0])

    def test_an_undocumented_option_is_still_a_failure_in_the_compact_layout(self):
        problems = self.drift(self.COMPACT + "    --cleanup <S>     Drain deadline\n")
        self.assertEqual(1, len(problems))
        self.assertIn("--cleanup", problems[0])

    def test_a_reader_that_saw_no_entries_would_be_reported_not_silent(self):
        """Why narrowing is safe to attempt: the page half of the comparison catches a blind one."""
        problems = self.drift("Options:\n")
        self.assertEqual(1, len(problems))
        self.assertIn("--max-active", problems[0])
        self.assertIn("absent from executable help", problems[0])

    def test_the_reader_takes_the_option_out_of_its_entry_and_not_its_description(self):
        self.assertEqual({"--max-active"}, checker.help_options(self.HELP))

    def test_a_value_list_under_an_entry_is_not_an_option(self):
        """clap prints possible values as `- name:` lines, which a dash-led reader could take."""
        self.assertEqual(
            {"--mode"},
            checker.help_options(
                "Options:\n      --mode <MODE>\n          Workload\n\n"
                "          Possible values:\n          - signalling:      Bodyless\n"
                "          - generated-media: Deterministic\n"
            ),
        )

    def drift(self, help_text: str) -> list[str]:
        return checker.help_drift(self.DOCUMENT, self.ROOT, {"load-responder": help_text})


if __name__ == "__main__":
    unittest.main()
