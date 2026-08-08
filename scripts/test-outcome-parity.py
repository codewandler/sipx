#!/usr/bin/env python3
"""Tests for check-outcome-parity.py, the guard that holds a command's outcomes to one field set.

Every rule is reversed on a fabricated crate, because the failure this guard exists to prevent is
silent by construction: a check that reads no chains reports perfect parity, and perfect parity is
exactly what a healthy repository reports. `X-117`'s report index, `X-38`'s surface checker and
`X-120`'s kernel guard were all believed while observing nothing, so the tests that matter most
here are the ones that make the guard *fail*.

The false-positive direction is tested too. A field that genuinely belongs to one ending — a
registrar's lease, an error string — must not fire, or the second person to hit it deletes the
step rather than the field.

And the blind spot is asserted rather than assumed: the reader cannot see a field added through a
binding, and there is a test that this shows up in the unattributed count instead of being
silently dropped, because a disclaimed limit nobody counts is a limit nobody discovers.
"""

import importlib.util
import pathlib
import sys
import textwrap
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent


def load_module():
    """Import check-outcome-parity.py, whose hyphen keeps it out of the normal import path."""
    # `scripts/` holds no package, so a cached `__pycache__` here is untracked litter in a
    # directory that otherwise contains only source.
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location(
        "check_outcome_parity", ROOT / "scripts" / "check-outcome-parity.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


guard = load_module()


class Crate:
    """A throwaway `crates/sipx-cli/src` the reader can be pointed at."""

    def __init__(self, root: pathlib.Path):
        self.root = root
        self.source = root / guard.COMMANDS
        self.source.mkdir(parents=True, exist_ok=True)

    def module(self, name: str, body: str) -> "Crate":
        (self.source / f"{name}.rs").write_text(textwrap.dedent(body), encoding="utf-8")
        return self

    def read(self):
        return guard.read_commands(self.root)

    def problems(self) -> list[str]:
        """The parity findings, or the reason the reader refused to report any.

        `unused_exemptions` is deliberately not here: it holds this repository's table against
        this repository's commands, so every fabricated crate would trip all of it and drown the
        rule each test is actually asserting. It has its own tests below.
        """
        by_command, _ = self.read()
        return guard.scope_problems(by_command) or guard.parity_problems(by_command)

    def unused(self) -> list[str]:
        by_command, _ = self.read()
        return guard.unused_exemptions(by_command)


def crate(case: unittest.TestCase) -> Crate:
    import tempfile

    directory = tempfile.TemporaryDirectory()
    case.addCleanup(directory.cleanup)
    return Crate(pathlib.Path(directory.name))


#: A command whose two outcomes agree.
BALANCED = """
    fn ok() -> Report {
        Report::new().text("status", "done").text("subject", subject)
    }
    fn bad() -> Report {
        Report::new().text("status", "failed").text("subject", subject)
    }
"""


def populated(case: unittest.TestCase) -> Crate:
    """A fixture crate already over the plausibility floor.

    The floor exists so that a reader which has stopped reading cannot report parity over nothing,
    which means a two-line fixture trips it. Filling the crate first is what keeps each test
    asserting its own rule rather than the floor.
    """
    return crate(case).module("filler_one", BALANCED).module("filler_two", BALANCED)


class TheRepositoryItself(unittest.TestCase):
    """The state the gate demands, asserted here so a failure names which half broke."""

    def setUp(self):
        self.by_command, self.unattributed = guard.read_commands(ROOT)

    def test_the_commands_agree(self):
        self.assertEqual([], guard.parity_problems(self.by_command))

    def test_no_exemption_has_outlived_its_reason(self):
        self.assertEqual([], guard.unused_exemptions(self.by_command))

    def test_the_reader_still_understands_the_crate(self):
        self.assertEqual([], guard.scope_problems(self.by_command))

    def test_it_compares_the_commands_that_have_sibling_outcomes(self):
        """Pinned here rather than in the checker's floors, which are deliberately slack."""
        self.assertEqual(
            ["answer", "dial", "register"], sorted(guard.compared(self.by_command))
        )

    def test_every_register_outcome_names_its_address_of_record(self):
        """`P-25`'s gap and `P-28`'s fix, asserted on the code rather than on a running process."""
        for record in guard.compared(self.by_command)["register"]:
            with self.subTest(outcome=record.label):
                self.assertIn("aor", [field.name for field in record.fields])

    def test_every_dial_outcome_names_the_peer_it_called(self):
        """The same gap in the sibling command, which is what this checker found."""
        for record in guard.compared(self.by_command)["dial"]:
            with self.subTest(outcome=record.label):
                self.assertIn("peer", [field.name for field in record.fields])

    def test_the_blind_spot_is_real_and_counted(self):
        """`purr` and `flow` are added through a binding. The count is how a reader learns that."""
        names = {skipped.name for skipped in self.unattributed}
        self.assertIn("purr", names)
        self.assertIn("flow", names)


class TheChainReader(unittest.TestCase):
    """Everything downstream is derived from this, so a chain it misreads is a rule it skips."""

    def read_one(self, body: str):
        records, unattributed = guard.read_records("dial", textwrap.dedent(body))
        return records, unattributed

    def test_it_follows_a_chain_across_lines_and_nested_calls(self):
        records, _ = self.read_one(
            """
            let report = Report::new()
                .text("status", "answered")
                .number(
                    "duration_ms",
                    i64::try_from(progress.elapsed().as_millis()).unwrap_or(0),
                )
                .boolean("heard_audio", total != 0);
            """
        )
        self.assertEqual(
            ["status", "duration_ms", "heard_audio"],
            [field.name for field in records[0].fields],
        )

    def test_a_string_argument_containing_a_parenthesis_does_not_end_the_chain(self):
        records, _ = self.read_one(
            """
            Report::new().text("status", "failed").text("error", "bad ) input").text("peer", uri)
            """
        )
        self.assertEqual(
            ["status", "error", "peer"], [field.name for field in records[0].fields]
        )

    def test_an_escaped_quote_does_not_end_a_string(self):
        records, _ = self.read_one(
            r"""
            Report::new().text("status", "x").text("reason", "he said \"no\"").text("peer", uri)
            """
        )
        self.assertEqual(
            ["status", "reason", "peer"], [field.name for field in records[0].fields]
        )

    def test_a_character_literal_is_not_a_string(self):
        records, _ = self.read_one(
            """
            Report::new().text("status", "x").text("sep", format!("{}", ',')).text("peer", uri)
            """
        )
        self.assertEqual(
            ["status", "sep", "peer"], [field.name for field in records[0].fields]
        )

    def test_a_comment_between_calls_does_not_end_the_chain(self):
        records, _ = self.read_one(
            """
            Report::new()
                .text("status", "failed")
                // `peer` on every outcome (`P-28`).
                .text("peer", uri)
            """
        )
        self.assertEqual(["status", "peer"], [field.name for field in records[0].fields])

    def test_emit_ends_a_chain_rather_than_naming_a_field(self):
        records, _ = self.read_one(
            """
            Report::new().text("status", "x").emit(format);
            """
        )
        self.assertEqual(["status"], [field.name for field in records[0].fields])

    def test_a_literal_status_labels_the_outcome(self):
        records, _ = self.read_one("""Report::new().text("status", "registered")""")
        self.assertEqual("registered", records[0].label)

    def test_an_exit_variant_labels_the_outcome_it_exits_under(self):
        records, _ = self.read_one(
            """Report::new().text("status", Exit::Timeout.as_str())"""
        )
        self.assertEqual("timeout", records[0].label)

    def test_a_computed_status_is_labelled_by_position_rather_than_guessed(self):
        records, _ = self.read_one(
            """
            let report = Report::new().text("status", exit.as_str());
            """
        )
        self.assertTrue(records[0].is_outcome)
        self.assertEqual("dial.rs:2", records[0].label)

    def test_a_chain_without_a_status_is_a_fragment_and_not_an_outcome(self):
        """`counters::report` builds one. Comparing it with a result would report nonsense."""
        records, _ = self.read_one(
            """Report::new().boolean("any_loss", counts.any_loss())"""
        )
        self.assertFalse(records[0].is_outcome)

    def test_a_field_added_through_a_binding_is_counted_as_unattributed(self):
        _, unattributed = self.read_one(
            """
            let mut report = Report::new().text("status", "registered");
            if outbound {
                report = report.boolean("flow", agent.flow_accepted());
            }
            """
        )
        self.assertEqual(["flow"], [skipped.name for skipped in unattributed])

    def test_a_test_module_is_not_read(self):
        """A fixture building a lopsided report on purpose must not be anybody's outcome."""
        records, _ = guard.read_records(
            "dial",
            guard.command_source(
                textwrap.dedent(
                    """
                    Report::new().text("status", "answered").text("peer", uri)

                    #[cfg(test)]
                    mod tests {
                        fn sample() -> Report {
                            Report::new().text("status", "answered")
                        }
                    }
                    """
                )
            ),
        )
        self.assertEqual(1, len(records))

    def test_the_builder_module_is_not_a_command(self):
        fixture = crate(self)
        fixture.module(guard.BUILDER.removesuffix(".rs"), BALANCED)
        by_command, _ = fixture.read()
        self.assertEqual({}, by_command)


class TheRule(unittest.TestCase):
    """What the gate step actually asserts, reversed on a crate written for the purpose."""

    def test_a_field_on_one_outcome_and_not_its_sibling_is_a_finding(self):
        problems = (
            populated(self)
            .module(
                "call",
                """
                fn answered() -> Report {
                    Report::new().text("status", "answered").text("peer", uri)
                }
                fn refused() -> Report {
                    Report::new().text("status", "refused")
                }
                """,
            )
            .problems()
        )
        self.assertEqual(1, len(problems), problems)
        self.assertIn("`peer`", problems[0])
        self.assertIn("`answered`", problems[0])
        self.assertIn("`refused`", problems[0])

    def test_outcomes_that_agree_are_not_a_finding(self):
        problems = (
            populated(self)
            .module(
                "call",
                """
                fn answered() -> Report {
                    Report::new().text("status", "answered").text("peer", uri)
                }
                fn refused() -> Report {
                    Report::new().text("status", "refused").text("peer", uri)
                }
                """,
            )
            .problems()
        )
        self.assertEqual([], problems)

    def test_a_declared_outcome_specific_field_does_not_fire(self):
        """A registrar's lease exists only where a registrar answered. So does `error`."""
        problems = (
            populated(self)
            .module(
                "call",
                """
                fn answered() -> Report {
                    Report::new().text("status", "answered").text("peer", uri)
                }
                fn refused() -> Report {
                    Report::new()
                        .text("status", "refused")
                        .text("peer", uri)
                        .text("error", cause)
                }
                """,
            )
            .problems()
        )
        self.assertEqual([], problems)

    def test_a_command_with_one_outcome_is_not_evidence_of_parity(self):
        by_command, _ = (
            populated(self)
            .module("only", """Report::new().text("status", "peer").text("name", n)""")
            .read()
        )
        self.assertNotIn("only", guard.compared(by_command))
        self.assertIn("only", by_command)

    def test_a_record_written_before_the_command_has_a_subject_is_not_compared(self):
        """`answer`'s listener announcement, which can name nothing a call decides."""
        by_command, _ = guard.read_commands(ROOT)
        labels = [record.label for record in guard.compared(by_command)["answer"]]
        self.assertNotIn("listening", labels)

    def test_the_exemption_covers_that_record_and_nothing_wider(self):
        """Excluding a record is the strongest exemption here, so it must be exact pairs."""
        self.assertEqual(
            {("answer", "listening"), ("answer", "interrupted")},
            set(guard.WITHOUT_A_CALL),
        )
        self.assertNotIn(("dial", "interrupted"), guard.WITHOUT_A_CALL)


class ThePublicReference(unittest.TestCase):
    """The half of `P-28`'s fourth row a script can hold: the page enumerates what is emitted."""

    def test_every_field_this_repository_emits_is_documented(self):
        by_command, _ = guard.read_commands(ROOT)
        self.assertEqual([], guard.documentation_problems(by_command, ROOT))

    def test_an_undocumented_field_is_a_finding(self):
        fixture = populated(self).module(
            "call",
            """
            fn answered() -> Report {
                Report::new().text("status", "answered").text("undocumented_field", x)
            }
            """,
        )
        page = fixture.root / guard.REFERENCE
        page.parent.mkdir(parents=True, exist_ok=True)
        page.write_text("`status` and `subject` are reported.\n", encoding="utf-8")
        by_command, _ = fixture.read()
        problems = guard.documentation_problems(by_command, fixture.root)
        self.assertEqual(1, len(problems), problems)
        self.assertIn("`undocumented_field`", problems[0])

    def test_a_documented_field_no_chain_names_is_not_a_finding(self):
        """The reverse direction would fail on the helper contributions this cannot see."""
        fixture = populated(self)
        page = fixture.root / guard.REFERENCE
        page.parent.mkdir(parents=True, exist_ok=True)
        page.write_text(
            "`status`, `subject`, and `negotiated_transport` are reported.\n", encoding="utf-8"
        )
        by_command, _ = fixture.read()
        self.assertEqual([], guard.documentation_problems(by_command, fixture.root))

    def test_a_tree_with_no_reference_page_reports_nothing_rather_than_everything(self):
        by_command, _ = populated(self).read()
        self.assertEqual(
            [], guard.documentation_problems(by_command, pathlib.Path("/nonexistent"))
        )


class TheSilenceGuards(unittest.TestCase):
    """The half that makes an empty reading a failure instead of a clean bill of health."""

    def test_a_tree_with_no_commands_at_all_is_a_finding(self):
        problems = crate(self).problems()
        self.assertEqual(1, len(problems), problems)
        self.assertIn("no command with sibling outcomes", problems[0])

    def test_a_reader_that_finds_chains_but_no_fields_is_a_finding(self):
        """A `Report::new()` whose chain is never walked reads as a tree with nothing to compare."""
        problems = (
            crate(self)
            .module("one", "let a = Report::new();\nlet b = Report::new();")
            .module("two", "let c = Report::new();")
            .problems()
        )
        self.assertIn("no command with sibling outcomes", problems[0])

    def test_too_few_commands_is_a_finding(self):
        problems = crate(self).module("only", BALANCED).problems()
        self.assertIn("sibling outcomes for only 1 command", " ".join(problems))

    def test_an_exemption_nothing_needs_is_a_finding(self):
        """A table of unfalsifiable reasons is how the next reader learns to skim this one."""
        unused = populated(self).unused()
        self.assertEqual(
            sorted(guard.OUTCOME_SPECIFIC),
            sorted(
                problem.split("`")[1]
                for problem in unused
                if problem.startswith("OUTCOME_SPECIFIC")
            ),
            "a crate reporting none of the declared fields must flag every entry",
        )

    def test_a_declared_record_that_no_longer_exists_is_a_finding(self):
        gone = [
            problem for problem in populated(self).unused() if "WITHOUT_A_CALL" in problem
        ]
        self.assertEqual(len(guard.WITHOUT_A_CALL), len(gone), gone)


class TheEntryPoint(unittest.TestCase):
    """The two modes the gate and a reader use, and the exit codes they are read by."""

    def test_check_passes_on_this_repository(self):
        self.assertEqual(0, guard.main(["--check"]))

    def test_explain_prints_every_record_and_the_blind_spot(self):
        import contextlib
        import io

        printed = io.StringIO()
        with contextlib.redirect_stdout(printed):
            self.assertEqual(0, guard.main(["--explain"]))
        output = printed.getvalue()
        self.assertIn("no call yet", output)
        self.assertIn("fragment", output)
        self.assertIn("blind spot", output)

    def test_check_fails_on_an_unbalanced_crate(self):
        fixture = (
            populated(self)
            .module(
                "call",
                """
                fn answered() -> Report {
                    Report::new().text("status", "answered").text("peer", uri)
                }
                fn refused() -> Report {
                    Report::new().text("status", "refused")
                }
                """,
            )
        )
        import contextlib
        import io

        reported = io.StringIO()
        with contextlib.redirect_stderr(reported):
            self.assertEqual(1, guard.main(["--check", "--root", str(fixture.root)]))
        self.assertIn("`peer` is reported by `answered`", reported.getvalue())

    def test_neither_mode_is_refused_rather_than_assumed(self):
        with self.assertRaises(SystemExit):
            guard.main([])


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
