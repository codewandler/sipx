#!/usr/bin/env python3
"""Tests for coverage-report.py, the one published number this repository refuses to act on.

A coverage figure is the most dangerous kind of measurement here, because it looks like the thing
`docs/maturity.md` says it is not: *"Nothing here measures whether the tests are good, only that
they pass."* Coverage does not fix that. It bounds it — it says which lines the suite never
executes, and says nothing whatever about whether executing them proved anything. `X-36` is the
scar: a test that ran the code it was named for and could not detect the reversal of its own
invariant. It had coverage.

So this suite is organised around the three ways the number could quietly become a lie, and every
one of them has a reversed fixture:

1. **Transcription.** `docs/roadmap.md`'s Status block said "941 tests pass" through four releases
   in which the real number went past 1300. A percentage is worse than a count, because nobody can
   sanity-check it by eye. The rules tested here are that the report is rendered from
   `docs/coverage/measurement.json` and byte-compared, that the measurement may not carry a
   percentage of its own — percentages are arithmetic done at render time — and that changing a
   count changes the page.
2. **Becoming a gate.** `docs/roadmap.md` refuses a v1 gate built on coverage in as many words, and
   the story that produced this file says the number is never asserted. A threshold is one flag
   away at all times, so a zero-coverage measurement is tested to check *green*, and both the
   checker and the CI job are read for a `--fail-under`.
3. **Measuring itself.** Test code is executed by definition. A figure that counts `tests/` is a
   figure that rises when you write more tests and never falls when you write untested code, which
   is `X-36`'s shape in a new place. So the exclusions are tested twice: that every one is applied
   to the measurement command, and that every one is named on the same page as the number — which
   is the story's Acceptance verbatim.

The gate registration is tested here rather than left to `gate.py --check`, because the two say
different things. `--check` says the job is *accounted for*; these say the account is the one the
story asked for, with a reason attached, and that the cheap half of the work — rendering and
comparing — is a local gate step rather than something only a hosted runner ever does.
"""

import importlib.util
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"

#: The CI job that measures. Named once, because both this file and the gate refer to it.
COVERAGE_JOB = "coverage"

#: How a threshold would arrive. `cargo llvm-cov` spells all three the same way, and any of them
#: turns a published number into a build gate — which is the thing `docs/roadmap.md` refuses.
THRESHOLD_FLAGS = ("--fail-under-lines", "--fail-under-functions", "--fail-under-regions")


def load_module(name, filename):
    """Import a hyphenated script, which is not a legal module name and so not importable."""
    # `scripts/` holds no package, so a cached `__pycache__` here is untracked litter in a
    # directory that otherwise contains only source.
    sys.dont_write_bytecode = True
    path = ROOT / "scripts" / filename
    if not path.is_file():
        raise RuntimeError(
            f"{path.relative_to(ROOT)} does not exist, so the published coverage figure has no "
            f"generator and any number in the tree was typed by somebody"
        )
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Lazy:
    """The generator, imported on first use rather than at collection.

    Deliberately lazy. Loading it at import time would make one missing file end the whole run, and
    the rules in this file are separable: whether CI measures at all, whether the gate accounts for
    the job, and whether the figure is rendered are three different failures that should be able to
    arrive in the same report. This is also what makes the first run against a tree that has none of
    it useful rather than a single stack trace.
    """

    def __init__(self, name, filename):
        self._name = name
        self._filename = filename
        self._module = None

    def __getattr__(self, attribute):
        if self._module is None:
            self._module = load_module(self._name, self._filename)
        return getattr(self._module, attribute)


gate = load_module("gate", "gate.py")
coverage = Lazy("coverage_report", "coverage-report.py")


def measurement(lines=(70, 100), branches=(30, 60), functions=(8, 10), crates=None):
    """A well-formed measurement, with the counts a caller cares about and defaults elsewhere."""
    return {
        "tool": "cargo-llvm-cov",
        "tool_version": "cargo-llvm-cov 0.0.0",
        "toolchain": "rustc 0.0.0-nightly",
        "measured_at": "2026-01-01",
        "commit": "0" * 40,
        "command": ["cargo", "llvm-cov", "--workspace"],
        "excluded": [{"pattern": pattern, "why": why} for pattern, why in coverage.EXCLUDED],
        "totals": counts(lines, branches, functions),
        "crates": crates
        if crates is not None
        else {"sipx-sip": counts(lines, branches, functions)},
    }


def counts(lines, branches, functions):
    covered_lines, total_lines = lines
    covered_branches, total_branches = branches
    covered_functions, total_functions = functions
    return {
        "lines": {"covered": covered_lines, "total": total_lines},
        "branches": {"covered": covered_branches, "total": total_branches},
        "functions": {"covered": covered_functions, "total": total_functions},
    }


class TheJobIsRegistered(unittest.TestCase):
    """Acceptance: a CI job measures, and `gate.py` accounts for it rather than omitting it."""

    def setUp(self):
        self.jobs = gate.parse_workflow(WORKFLOW.read_text())

    def test_ci_defines_a_coverage_job(self):
        self.assertIn(
            COVERAGE_JOB,
            self.jobs,
            f"{WORKFLOW.name} defines no `{COVERAGE_JOB}` job, so nothing measures what the suite "
            f"reaches",
        )

    def test_the_job_measures_with_cargo_llvm_cov(self):
        runs = " ".join(self.jobs[COVERAGE_JOB].runs)
        self.assertIn("cargo-llvm-cov", runs, "the job never installs the measurement tool")
        self.assertIn(
            coverage.MEASURE_FLAG,
            runs,
            f"the `{COVERAGE_JOB}` job does not run the generator's {coverage.MEASURE_FLAG} mode, "
            f"so whatever it measures is not what the published figure is rendered from",
        )

    def test_the_job_publishes_the_result_as_a_build_artifact(self):
        uses = " ".join(self.jobs[COVERAGE_JOB].uses)
        self.assertIn(
            "actions/upload-artifact",
            uses,
            "the measurement is taken and then thrown away; the story asks for it as an artifact",
        )

    def test_the_gate_registers_the_job_with_a_reason(self):
        mirrored = {step.ci_job for step in gate.gate_steps("0.0.0")}
        self.assertTrue(
            COVERAGE_JOB in gate.NOT_RUN_LOCALLY or COVERAGE_JOB in mirrored,
            f"`{COVERAGE_JOB}` is neither a gate step nor in NOT_RUN_LOCALLY, so the gate can omit "
            f"it silently",
        )
        if COVERAGE_JOB in gate.NOT_RUN_LOCALLY:
            self.assertTrue(
                gate.NOT_RUN_LOCALLY[COVERAGE_JOB].strip(),
                "a NOT_RUN_LOCALLY entry with no reason is an omission with a name",
            )

    def test_the_cheap_half_is_a_local_gate_step(self):
        """Rendering and comparing needs no toolchain, so it belongs where an implementor runs it."""
        steps = [
            step
            for step in gate.gate_steps("0.0.0")
            if step.command and step.command[0].endswith("coverage-report.py")
        ]
        self.assertTrue(
            steps,
            "no gate step runs the coverage report checker, so a hand-edited figure reaches CI "
            "before anything notices",
        )
        self.assertTrue(
            any("--check" in step.command for step in steps),
            "the gate step does not run the checker in --check mode",
        )

    def test_the_gate_agrees_with_ci(self):
        done = subprocess.run(
            [str(ROOT / "scripts" / "gate.py"), "--check"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)


class NothingGatesOnTheNumber(unittest.TestCase):
    """`docs/roadmap.md` refuses a v1 gate built on coverage. This is what holds it to that."""

    def test_a_measurement_that_covers_nothing_still_checks_green(self):
        data = measurement(lines=(0, 100), branches=(0, 60), functions=(0, 10))
        self.assertEqual(coverage.schema_problems(data), [])
        with report_of(data) as (measurement_path, report_path):
            self.assertEqual(coverage.check(measurement_path, report_path), 0)

    def test_the_checker_carries_no_threshold(self):
        source = (ROOT / "scripts" / "coverage-report.py").read_text()
        for flag in THRESHOLD_FLAGS:
            self.assertNotIn(flag, source, f"{flag} turns the figure into a build gate")

    def test_the_ci_job_carries_no_threshold(self):
        jobs = gate.parse_workflow(WORKFLOW.read_text())
        runs = " ".join(jobs[COVERAGE_JOB].runs)
        for flag in THRESHOLD_FLAGS:
            self.assertNotIn(flag, runs, f"{flag} makes a coverage drop fail the build")

    def test_the_measure_command_carries_no_threshold(self):
        argv = " ".join(coverage.measure_command(pathlib.Path("/tmp/out")))
        for flag in THRESHOLD_FLAGS:
            self.assertNotIn(flag, argv)


class TheFigureIsGeneratedAndNotTyped(unittest.TestCase):
    """Acceptance: generated into the docs, never transcribed. A typed percentage fails."""

    def test_percentages_are_arithmetic_over_the_counts(self):
        self.assertEqual(coverage.percentage(1, 4), "25.00%")
        self.assertEqual(coverage.percentage(0, 100), "0.00%")
        self.assertEqual(coverage.percentage(100, 100), "100.00%")

    def test_nothing_is_reported_when_there_is_nothing_to_report(self):
        """Zero of zero is not zero percent, and printing `0.00%` for it would be a claim."""
        self.assertEqual(coverage.percentage(0, 0), coverage.NO_DATA)

    def test_a_changed_count_changes_the_published_figure(self):
        """The one property that separates a rendered number from a transcribed one."""
        before = coverage.render(measurement(lines=(70, 100)))
        after = coverage.render(measurement(lines=(71, 100)))
        self.assertNotEqual(before, after)
        self.assertIn("70.00%", before)
        self.assertIn("71.00%", after)
        self.assertNotIn("71.00%", before)

    def test_a_measurement_may_not_carry_its_own_percentage(self):
        """A stored percentage is a number somebody could type. The counts are the record."""
        data = measurement()
        data["totals"]["lines"]["percent"] = 99.9
        problems = coverage.schema_problems(data)
        self.assertTrue(problems)
        self.assertIn("percent", " ".join(problems))

    def test_a_hand_edited_record_stops_the_tables_adding_up(self):
        """The page is rendered from the record, so editing the record moves the page with it."""
        data = measurement()
        data["crates"]["sipx-sip"]["lines"]["covered"] += 1
        problems = coverage.schema_problems(data)
        self.assertTrue(problems)
        self.assertIn("would not add up", " ".join(problems))

    def test_a_hand_edited_report_fails_the_check(self):
        with report_of(measurement()) as (measurement_path, report_path):
            report_path.write_text(report_path.read_text().replace("70.00%", "97.00%"))
            self.assertEqual(coverage.check(measurement_path, report_path), 1)

    def test_a_missing_report_fails_the_check(self):
        with report_of(measurement()) as (measurement_path, report_path):
            report_path.unlink()
            self.assertEqual(coverage.check(measurement_path, report_path), 1)

    def test_a_malformed_measurement_is_named_rather_than_raised(self):
        """Whoever broke it needs a sentence, not a traceback out of the renderer."""
        data = measurement()
        del data["totals"]["branches"]
        self.assertTrue(coverage.schema_problems(data))
        with report_of(measurement()) as (measurement_path, report_path):
            measurement_path.write_text(json.dumps(data))
            self.assertEqual(coverage.check(measurement_path, report_path), 1)

    def test_the_real_report_is_current(self):
        self.assertEqual(coverage.check(coverage.MEASUREMENT, coverage.REPORT), 0)


class TheFigureStatesItsLimits(unittest.TestCase):
    """Acceptance: what it excludes is stated on the same surface that states the number."""

    def test_every_exclusion_is_named_on_the_page(self):
        page = coverage.render(measurement())
        for pattern, why in coverage.EXCLUDED:
            self.assertIn(pattern, page, f"the page does not say it excludes {pattern}")
            self.assertIn(why, page, f"the page does not say why it excludes {pattern}")

    def test_every_exclusion_is_applied_to_the_measurement(self):
        argv = coverage.measure_command(pathlib.Path("/tmp/out"))
        applied = " ".join(argv)
        for pattern, _ in coverage.EXCLUDED:
            self.assertIn(
                pattern,
                applied,
                f"{pattern} is published as excluded and the measurement does not exclude it, "
                f"which is a page describing a measurement nobody took",
            )

    def test_the_page_refuses_to_be_read_as_a_quality_claim(self):
        page = coverage.render(measurement())
        for phrase in coverage.DISCLAIMED:
            self.assertIn(phrase, page)

    def test_the_run_summary_is_written_where_ci_reads_it(self):
        """The CI job cats this file by name, so nothing writing it is a red push and not a bug."""
        data = measurement()
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            coverage.record(data, root / "out", root / "measurement.json")
            written = (root / "out" / coverage.SUMMARY_NAME).read_text()
        step = " ".join(gate.parse_workflow(WORKFLOW.read_text())[COVERAGE_JOB].runs)
        self.assertIn(coverage.SUMMARY_NAME, step, "no CI step publishes the run summary")
        self.assertIn("70.00%", written)
        for phrase in ("Nothing gates on this", "not whether executing it proved anything"):
            self.assertIn(phrase, written)

    def test_the_page_says_which_commit_it_describes(self):
        data = measurement()
        data["commit"] = "a" * 40
        self.assertIn("a" * 40, coverage.render(data))
        self.assertIn(data["measured_at"], coverage.render(data))


class report_of:
    """A temporary measurement and the report generated from it, for the reversed fixtures."""

    def __init__(self, data):
        self.data = data

    def __enter__(self):
        self.directory = tempfile.TemporaryDirectory()
        root = pathlib.Path(self.directory.name)
        self.measurement_path = root / "measurement.json"
        self.report_path = root / "coverage.md"
        self.measurement_path.write_text(json.dumps(self.data, indent=2) + "\n")
        coverage.write(self.measurement_path, self.report_path)
        return self.measurement_path, self.report_path

    def __exit__(self, *_):
        self.directory.cleanup()


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
