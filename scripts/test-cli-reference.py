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


if __name__ == "__main__":
    unittest.main()
