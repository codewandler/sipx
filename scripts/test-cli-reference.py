#!/usr/bin/env python3
"""Tests for the executable CLI-reference drift check."""

from __future__ import annotations

import importlib.util
import pathlib
import re
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


class TheFlagTableReader(unittest.TestCase):
    """`X-132`: a command's documented flags are the rows of a table, not every `|` line.

    The reader used to take a `` `--flag` `` out of the first cell of any line in the section that
    began with `|`, and never asked what those lines belonged to. A GFM table needs a delimiter row
    and cannot interrupt a paragraph, so `|` lines written under prose are paragraph text: pipes
    and all, on the rendered page. Finishing `X-129` put the sizing paragraph between two rows of
    `load-responder`'s table, and the six flags below it — `--transport`, `--local` and `--mode`
    among them — were documented to this checker and a wall of pipes to an operator for the length
    of one release.

    Narrowing a reader is how one goes quiet, so the last two rows are the property that makes it
    safe to attempt: `help_drift` compares both ways, so a page reader that stopped seeing rows is
    reported by the executable half of the same comparison rather than passing silently. The live
    page is held against the wide reader it replaced, because agreement between the two is exactly
    the statement that no command section carries a row outside a table.
    """

    #: A root help naming the one command these fixtures describe.
    ROOT = "Commands:\n  load-responder  Answer a load\n  help  Show this message\n"

    HELP = """\
OPTIONS:
    --max-active <N>  Ceiling on dialogs
    --transport <T>   Must be `udp`
    -h, --help        Help
"""

    #: The shape that reached the published page: a table opened, prose written through it, and the
    #: remaining rows resumed beneath that prose with no delimiter row of their own.
    SPLICED = """\
# CLI reference

## `sipx load-responder`

| Flag | Meaning |
|---|---|
| `--max-active <N>` | Ceiling on dialogs |

**Sizing `--max-active` against a generator.** Give it headroom over the generator's
`--concurrency` rather than matching it.
| `--transport <T>` | Must be `udp` |
"""

    #: The same flags, repaired the way the page was: one table, the paragraph below it.
    ONE_TABLE = """\
# CLI reference

## `sipx load-responder`

| Flag | Meaning |
|---|---|
| `--max-active <N>` | Ceiling on dialogs |
| `--transport <T>` | Must be `udp` |

**Sizing `--max-active` against a generator.** Give it headroom over the generator's
`--concurrency` rather than matching it.
"""

    def test_rows_resuming_after_a_paragraph_are_not_documented_flags(self):
        problems = self.drift(self.SPLICED)
        self.assertEqual(1, len(problems))
        self.assertIn("--transport", problems[0])
        self.assertIn("is not documented", problems[0])

    def test_the_same_rows_gathered_into_one_table_are_documented(self):
        self.assertEqual([], self.drift(self.ONE_TABLE))

    def test_a_header_and_delimiter_under_prose_do_not_open_a_table_either(self):
        """A table cannot interrupt a paragraph, so a whole one written into it still renders flat."""
        interrupting = self.SPLICED.replace(
            "| `--transport <T>` | Must be `udp` |",
            "| Flag | Meaning |\n|---|---|\n| `--transport <T>` | Must be `udp` |",
        )
        problems = self.drift(interrupting)
        self.assertEqual(1, len(problems))
        self.assertIn("--transport", problems[0])

    def test_the_reader_takes_flags_from_the_body_rows_of_a_delimited_table(self):
        self.assertEqual(
            {"load-responder": {"--max-active"}}, checker.document_command_flags(self.SPLICED)
        )

    def test_an_alignment_colon_delimiter_still_opens_a_table(self):
        """The narrowing is a delimiter row, not one spelling of one: `|:---|---:|` is legal GFM."""
        aligned = self.ONE_TABLE.replace("|---|---|", "| :--- | ---: |")
        self.assertEqual([], self.drift(aligned))

    def test_a_reader_that_saw_no_rows_would_be_reported_not_silent(self):
        """Why narrowing is safe to attempt: the executable half catches a blind page reader."""
        self.assertEqual(
            [
                "load-responder: executable option `--max-active` is not documented",
                "load-responder: executable option `--transport` is not documented",
            ],
            self.drift("# CLI reference\n\n## `sipx load-responder`\n\nNo table here.\n"),
        )

    def test_no_command_section_of_the_public_page_carries_a_row_outside_a_table(self):
        """The rest of the page, since until now nothing had been asking.

        Where the narrowed reader and the wide one it replaced agree, every `|` line in a command
        section is a row of a real table. Where they differ, some line counts as documented here
        and renders as literal pipes to a reader.
        """
        document = checker.DOCUMENT.read_text(encoding="utf-8")
        self.assertEqual(self.wide_command_flags(document), checker.document_command_flags(document))

    @staticmethod
    def wide_command_flags(document: str) -> dict[str, set[str]]:
        """The reader this story replaced: a flag from the first cell of any `|`-leading line."""
        headings = list(
            re.finditer(r"^## `sipx ([a-z][a-z0-9-]*)(?:\s+[^`]*)?`\s*$", document, re.MULTILINE)
        )
        commands: dict[str, set[str]] = {}
        for index, heading in enumerate(headings):
            end = headings[index + 1].start() if index + 1 < len(headings) else len(document)
            flags: set[str] = set()
            for line in document[heading.end() : end].splitlines():
                if line.startswith("|"):
                    first_cell = line.strip().strip("|").split("|", 1)[0]
                    flags.update(re.findall(r"`(--[a-z][a-z0-9-]*)", first_cell))
            commands[heading.group(1)] = flags - checker.GLOBAL_OPTIONS
        return commands

    def drift(self, document: str) -> list[str]:
        return checker.help_drift(document, self.ROOT, {"load-responder": self.HELP})


class TheContractTableReader(unittest.TestCase):
    """`X-133`: the versioned-contract half reads rows, the way `X-132` made the flag half read them.

    `_document_json_contracts` counted every line between the region markers that began with `|`,
    held three cells and did not contain `---`, and never asked whether those lines formed a table.
    Measured while implementing `X-132`: splicing a paragraph directly above the last row of the
    repaired contract table left all five contracts documented to the checker, while a Markdown
    reader met that row as pipes at the end of a paragraph. The failure that hides is the one the
    region exists to catch — a versioned contract whose public row stopped rendering, with
    `json_drift` reporting agreement.

    The hand-rolled `---` skip went with the rewrite: it was doing the delimiter row's job by hand,
    and it silently dropped any legitimate row whose cells happened to carry a triple dash.
    """

    ACTUAL = {
        "sipx.test.v1": checker.JsonContract("dial", frozenset({"schema", "status"})),
        "sipx.other.v1": checker.JsonContract("probe", frozenset({"schema"})),
    }

    #: The measured shape: a table opened, a paragraph written through it, and the last row
    #: resumed beneath that paragraph with no delimiter row of its own.
    SPLICED = """\
# CLI reference

## The JSON contract

<!-- BEGIN cli-json-contracts -->
| Contract | Producer | Required structural fields |
|---|---|---|
| `sipx.test.v1` | `dial` | `schema`, `status` |

**Reading this table.** Every producer emits `schema` before anything else.
| `sipx.other.v1` | `probe` | `schema` |
<!-- END cli-json-contracts -->
"""

    #: The same contracts, repaired the way `X-132` repaired the flag table: one table, the
    #: paragraph below it.
    ONE_TABLE = """\
# CLI reference

## The JSON contract

<!-- BEGIN cli-json-contracts -->
| Contract | Producer | Required structural fields |
|---|---|---|
| `sipx.test.v1` | `dial` | `schema`, `status` |
| `sipx.other.v1` | `probe` | `schema` |

**Reading this table.** Every producer emits `schema` before anything else.
<!-- END cli-json-contracts -->
"""

    def test_a_row_spliced_under_prose_is_not_a_documented_contract(self):
        problems = checker.json_drift(self.SPLICED, self.ACTUAL)
        self.assertEqual(["versioned CLI contract `sipx.other.v1` is not documented"], problems)

    def test_the_same_rows_gathered_into_one_table_are_documented(self):
        self.assertEqual([], checker.json_drift(self.ONE_TABLE, self.ACTUAL))

    def test_a_header_and_delimiter_under_prose_do_not_open_a_table_either(self):
        """A table cannot interrupt a paragraph, so a whole one written into it still renders flat."""
        interrupting = self.SPLICED.replace(
            "| `sipx.other.v1` | `probe` | `schema` |",
            "| Contract | Producer | Required structural fields |\n|---|---|---|\n"
            "| `sipx.other.v1` | `probe` | `schema` |",
        )
        problems = checker.json_drift(interrupting, self.ACTUAL)
        self.assertEqual(["versioned CLI contract `sipx.other.v1` is not documented"], problems)

    def test_a_row_whose_cells_carry_a_triple_dash_is_still_a_row(self):
        """What the hand-rolled skip cost: the delimiter row is recognised, not guessed at."""
        dashed = self.ONE_TABLE.replace(
            "| `sipx.other.v1` | `probe` | `schema` |",
            "| `sipx.other.v1` | `probe` | `schema` --- and nothing else |",
        )
        self.assertEqual([], checker.json_drift(dashed, self.ACTUAL))

    def test_a_reader_that_saw_no_rows_would_be_reported_not_silent(self):
        """Why narrowing is safe to attempt: `json_drift` compares both ways."""
        self.assertEqual(
            [
                "versioned CLI contract `sipx.other.v1` is not documented",
                "versioned CLI contract `sipx.test.v1` is not documented",
            ],
            checker.json_drift(
                "# CLI reference\n\n<!-- BEGIN cli-json-contracts -->\n\nNo table here.\n"
                "<!-- END cli-json-contracts -->\n",
                self.ACTUAL,
            ),
        )

    def test_no_contract_row_of_the_public_page_sits_outside_a_table(self):
        """The live region, held against the wide reader this narrowing replaced.

        Where the two agree, every `|` line between the markers is a row of a real table. Where
        they differ, some line counts as documented here and renders as literal pipes to a reader.
        """
        document = checker.DOCUMENT.read_text(encoding="utf-8")
        self.assertEqual(
            self.wide_contracts(document), checker._document_json_contracts(document)
        )

    @staticmethod
    def wide_contracts(document: str) -> dict[str, object]:
        """The reader this story replaced: any three-celled `|` line between the markers."""
        try:
            body = document.split(checker.BEGIN_JSON, 1)[1].split(checker.END_JSON, 1)[0]
        except IndexError:
            return {}
        contracts: dict[str, object] = {}
        for line in body.splitlines():
            if not line.startswith("|") or "---" in line:
                continue
            cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
            if len(cells) != 3:
                continue
            contract = re.fullmatch(r"`(sipx\.[a-z0-9.-]+\.v[0-9]+)`", cells[0])
            producer = re.fullmatch(r"`([a-z][a-z0-9_-]*)`", cells[1])
            if contract is None or producer is None:
                continue
            contracts[contract.group(1)] = checker.JsonContract(
                producer.group(1), frozenset(re.findall(r"`([a-z][a-z0-9_]*)`", cells[2]))
            )
        return contracts


if __name__ == "__main__":
    unittest.main()
