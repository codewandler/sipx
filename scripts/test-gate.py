#!/usr/bin/env python3
"""Tests for the gate entry point, its drift check and its disk guard (stories `X-22`, `X-34`).

The gate is the contract for "before marking any story done", and for five days it was a list of
commands that omitted one CI runs. Every documented command passed, the `msrv` job was red, and
two releases shipped that did not build on the Rust version they advertise.

So the thing under test here is not "does the gate run". It is the ways a gate stops being worth
believing: a CI job the gate never learns about, an `AGENTS.md` gate block that grows its own copy
of the list and falls behind, and — `X-34` — a red result that was never about the tree at all.
All three are assertions about real repository files rather than about fixtures, because a check
that only ever sees its own fixtures is the same class of mistake one level up.
"""

import importlib.util
import pathlib
import sys
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
AGENTS = ROOT / "AGENTS.md"
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"

_gate = None


def gate():
    """Import gate.py lazily, so a test that reads only AGENTS.md still reports its own failure.

    The first two tests below are the ones that fail on the commit this story starts from, and
    what they say there has to be "the documented gate does not name the MSRV check" — not an
    import error about a script that does not exist yet.
    """
    global _gate
    if _gate is None:
        # `scripts/` holds no package, so a cached `__pycache__` here is untracked litter in a
        # directory that otherwise contains only source.
        sys.dont_write_bytecode = True
        spec = importlib.util.spec_from_file_location("gate", ROOT / "scripts" / "gate.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        _gate = module
    return _gate


def rust_version() -> str:
    return tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"][
        "rust-version"
    ]


class TheDocumentedGate(unittest.TestCase):
    """`AGENTS.md` is where an implementor learns what "done" costs. It has to be complete."""

    def setUp(self):
        self.text = AGENTS.read_text()

    def gate_section(self) -> str:
        """The `## The gate` section, read the way the checker reads it."""
        _, _, rest = self.text.partition("\n## The gate\n")
        self.assertTrue(rest, "AGENTS.md has no `## The gate` section")
        section, _, _ = rest.partition("\n## ")
        return section

    def test_the_gate_section_names_the_msrv_check(self):
        """The defect, stated as a test.

        CI has run an `msrv` job since the workspace was scaffolded. The gate never named it, so
        every documented command could pass on a tree that does not build on its own declared
        minimum — which is what happened, from v0.4.0 through v0.7.0.
        """
        section = self.gate_section().lower()
        self.assertIn(
            "msrv",
            section,
            "AGENTS.md's gate does not name the MSRV check, so an implementor can run every "
            "documented command, see green, and still break CI's `msrv` job",
        )

    def test_the_gate_block_transcribes_nothing(self):
        """The commands live in one place, and this section is not it.

        A gate block that lists commands is a copy, and the copy is what fell behind. The block
        may invoke the entry point and say nothing else.
        """
        problems = gate().documentation_problems(self.text)
        self.assertEqual([], problems)

    def test_the_msrv_version_is_not_written_here(self):
        """`rust-version` in `Cargo.toml` is the source; the gate reads it rather than repeating it."""
        self.assertNotIn(
            rust_version(),
            self.gate_section(),
            "AGENTS.md's gate section writes the MSRV version down a second time; it is derived "
            "from the workspace `rust-version`",
        )


class TheDriftCheck(unittest.TestCase):
    """`--check` has to fail when the gate and CI disagree — in either direction."""

    def setUp(self):
        self.gate = gate()
        self.jobs = self.gate.parse_workflow(WORKFLOW.read_text())
        self.steps = self.gate.gate_steps("1.0.0")

    def test_the_repository_itself_has_no_drift(self):
        self.assertEqual([], self.gate.drift_problems(self.jobs, self.steps))

    def test_a_new_ci_job_is_reported(self):
        """The defect's own shape: CI grows a job and nothing tells anyone.

        The job need not be runnable locally — it needs a decision. Either it becomes a gate step
        or it is named as deliberately remote, with a reason.
        """
        jobs = dict(self.jobs)
        jobs["audit"] = self.gate.Job("audit", runs=["cargo audit"])
        problems = self.gate.drift_problems(jobs, self.steps)
        self.assertTrue(
            any("audit" in p for p in problems),
            f"a CI job absent from the gate was accepted in silence; problems={problems}",
        )

    def test_a_new_command_in_a_covered_job_is_reported(self):
        """A job the gate already mirrors can still grow a second command."""
        jobs = {name: self.gate.Job(job.name, list(job.runs), list(job.uses)) for name, job in self.jobs.items()}
        jobs["test"].runs.append("cargo test --workspace --no-default-features")
        problems = self.gate.drift_problems(jobs, self.steps)
        self.assertTrue(
            any("no-default-features" in p for p in problems),
            f"a command only CI runs was accepted; problems={problems}",
        )

    def test_a_flag_ci_passes_and_the_gate_drops_is_reported(self):
        """The quiet direction: the gate runs the same command, but weaker.

        A local `cargo check` without `--all-targets` passes on a tree whose tests do not compile
        on the MSRV. That is a green gate and a red CI, which is the whole failure this story is
        about, one argument down.
        """
        steps = [
            s._replace(command=tuple(t for t in s.command if t != "--all-targets"))
            if s.name == "msrv"
            else s
            for s in self.steps
        ]
        problems = self.gate.drift_problems(self.jobs, steps)
        self.assertTrue(
            any("--all-targets" in p for p in problems),
            f"a gate step weaker than its CI job was accepted; problems={problems}",
        )

    def test_a_declared_difference_is_accepted(self):
        """`check-provenance.sh --history` is a real difference, and it is declared.

        The guard must not cost a legitimate difference anything, or the next person silences it
        by deleting the check.
        """
        provenance = [s for s in self.steps if s.ci_job == "provenance"]
        self.assertEqual(1, len(provenance))
        self.assertTrue(
            any(flag == "--history" for flag, _ in provenance[0].differs),
            "the provenance step's difference from CI is not declared",
        )
        self.assertTrue(all(why for _, why in provenance[0].differs), "a declared difference has no reason")

    def test_a_gate_step_with_no_ci_job_is_reported(self):
        """The other direction: a step nothing in CI backs is a check that only one person runs."""
        steps = list(self.steps) + [
            self.gate.Step("invented", "nonexistent", ("./scripts/nothing.sh",))
        ]
        problems = self.gate.drift_problems(self.jobs, steps)
        self.assertTrue(
            any("nonexistent" in p for p in problems),
            f"a gate step mirroring no CI job was accepted; problems={problems}",
        )

    def test_a_stale_exclusion_is_reported(self):
        """A job named as not-runnable-locally that CI no longer defines is a dead reason."""
        jobs = {name: job for name, job in self.jobs.items() if name != "soak"}
        problems = self.gate.drift_problems(jobs, self.steps)
        self.assertTrue(
            any("soak" in p for p in problems),
            f"a stale exclusion was accepted; problems={problems}",
        )

    def test_every_excluded_job_carries_a_reason(self):
        for name, why in self.gate.NOT_RUN_LOCALLY.items():
            with self.subTest(job=name):
                self.assertTrue(why.strip(), f"CI job `{name}` is excluded with no reason")


class TheMsrvStep(unittest.TestCase):
    """The step this story exists for, and the two ways it could still lie."""

    def setUp(self):
        self.gate = gate()

    def test_the_toolchain_is_read_from_the_workspace(self):
        self.assertEqual(
            self.gate.normalise_version(rust_version()),
            self.gate.normalise_version(self.gate.msrv_toolchain()),
        )

    def test_the_version_is_written_nowhere_in_the_gate(self):
        """Derived, not transcribed — in the script as well as in AGENTS.md."""
        self.assertEqual([], self.gate.version_literal_problems(rust_version()))

    def test_ci_pins_the_version_the_workspace_declares(self):
        """The `msrv` job's toolchain and `rust-version` are the same claim in two files."""
        jobs = self.gate.parse_workflow(WORKFLOW.read_text())
        self.assertEqual([], self.gate.toolchain_problems(jobs, rust_version()))

    def test_a_ci_pin_that_drifted_is_reported(self):
        jobs = self.gate.parse_workflow(WORKFLOW.read_text())
        jobs["msrv"] = self.gate.Job("msrv", uses=["dtolnay/rust-toolchain@1.99.0"])
        problems = self.gate.toolchain_problems(jobs, rust_version())
        self.assertTrue(
            any("1.99" in p for p in problems),
            f"a CI pin that no longer matches rust-version was accepted; problems={problems}",
        )

    def test_a_missing_toolchain_is_a_failure_and_says_how_to_fix_it(self):
        """Not a skip. A skipped MSRV step is indistinguishable from the bug it looks for."""
        problem = self.gate.missing_toolchain_problem([], "1.88.0")
        self.assertIsNotNone(problem, "a missing MSRV toolchain was treated as a pass")
        self.assertIn("rustup toolchain install 1.88.0", problem)

    def test_no_rustup_at_all_is_a_failure_and_says_how_to_fix_it(self):
        problem = self.gate.missing_toolchain_problem(None, "1.88.0")
        self.assertIsNotNone(problem, "a machine with no rustup was treated as a pass")
        self.assertIn("rustup", problem)

    def test_an_installed_toolchain_passes_however_it_is_spelled(self):
        """`rustup toolchain install 1.88` and `1.88.0` produce differently named toolchains."""
        for installed in (
            ["1.88.0-x86_64-unknown-linux-gnu"],
            ["1.88-x86_64-unknown-linux-gnu"],
            ["stable-x86_64-unknown-linux-gnu (active, default)", "1.88.0-x86_64-unknown-linux-gnu"],
        ):
            with self.subTest(installed=installed):
                self.assertIsNone(self.gate.missing_toolchain_problem(installed, "1.88.0"))


class TheWorkflowParser(unittest.TestCase):
    """The check is only as good as its reading of ci.yml, so the reading asserts itself."""

    def setUp(self):
        self.gate = gate()
        self.jobs = self.gate.parse_workflow(WORKFLOW.read_text())

    def test_it_finds_every_job(self):
        """A parser that silently finds nothing would report no drift, forever."""
        for name in ("fmt", "clippy", "test", "msrv", "features", "site", "provenance", "soak"):
            with self.subTest(job=name):
                self.assertIn(name, self.jobs)

    def test_it_reads_the_commands_a_job_runs(self):
        self.assertIn("cargo fmt --all --check", self.jobs["fmt"].runs)
        self.assertEqual(
            2,
            len([r for r in self.jobs["test"].runs if r.startswith("cargo")]),
            "the `test` job runs both a test command and an examples build",
        )

    def test_it_reads_block_scalars_without_swallowing_the_rest_of_the_job(self):
        """The fuzz job's `run: |` must not eat the step that follows it."""
        self.assertTrue(
            any("import-rfc4475-corpus.sh" in r for r in self.jobs["fuzz"].runs),
            "a block scalar swallowed the step after it",
        )

    def test_it_reads_the_actions_a_job_uses(self):
        self.assertIn("actions/checkout@v4", self.jobs["fmt"].uses)

    def test_it_reads_the_workflow_environment(self):
        """The gate builds with CI's flags, and reads them from CI rather than repeating them."""
        self.assertEqual(
            "-D warnings", self.gate.parse_workflow_env(WORKFLOW.read_text())["RUSTFLAGS"]
        )

    def test_a_workflow_it_cannot_read_is_an_error_not_an_empty_result(self):
        with self.assertRaises(ValueError):
            self.gate.parse_workflow("name: CI\non:\n  push:\n")


class TheDiskGuard(unittest.TestCase):
    """`X-34`: five red gates in one evening were a full disk, and every one read as a code defect.

    The near-miss is the reason this class exists. `X-28` was a correct merge, three of its
    integration gate's steps were red, and the one that looked real named a crate its diff never
    opened. It was one command away from being reverted. Cargo's messages in this state — a missing
    `.d` file, a fingerprint it could not write, an `.rlib` that "does not exist" — all read as
    compile errors, so the only defence is for the gate to refuse to produce a result it cannot
    stand behind.
    """

    GIB = 1024**3

    def setUp(self):
        self.gate = gate()

    # -- refusing to start ---------------------------------------------------------------------

    def test_a_disk_below_the_threshold_refuses_to_start(self):
        """The failing-first assertion: too little disk has to stop the gate, not colour it red."""
        required = self.gate.required_free_bytes(0)
        problem = self.gate.disk_problem(free=required - 1, required=required)
        self.assertIsNotNone(
            problem,
            "the gate started with less free space than a cold run has been measured to need; "
            "cargo will report a missing build artifact and a human will read it as a defect",
        )

    def test_the_refusal_names_disk_and_states_both_numbers(self):
        """A human must never have to guess which of the two numbers was the problem."""
        problem = self.gate.disk_problem(free=3 * self.GIB, required=25 * self.GIB)
        self.assertIsNotNone(problem)
        self.assertIn("disk", problem.lower(), "the refusal does not name disk")
        self.assertIn("3.0 GiB", problem, "the refusal does not state the actual free space")
        self.assertIn("25.0 GiB", problem, "the refusal does not state the threshold")

    def test_ample_free_space_is_not_a_problem(self):
        """The guard must cost a healthy machine nothing, or it gets deleted rather than fixed."""
        required = self.gate.required_free_bytes(0)
        self.assertIsNone(self.gate.disk_problem(free=required, required=required))
        self.assertIsNone(self.gate.disk_problem(free=required * 4, required=required))

    # -- the threshold is measured -------------------------------------------------------------

    def test_the_threshold_covers_every_size_ever_measured(self):
        """Derived from measurements, not chosen. That is the story's word: "rather than guessed"."""
        self.assertTrue(self.gate.MEASURED_GATE_TARGET_GIB, "no measurement backs the threshold")
        largest = max(self.gate.MEASURED_GATE_TARGET_GIB.values())
        self.assertGreaterEqual(
            self.gate.required_free_bytes(0),
            largest * self.GIB,
            "the guard would let a cold run start with less disk than a cold run has actually "
            "been measured to consume, which is the guess this story exists to remove",
        )

    def test_every_measurement_says_where_it_came_from(self):
        """A number with no provenance is a guess that has been rounded."""
        for what, gib in self.gate.MEASURED_GATE_TARGET_GIB.items():
            with self.subTest(measurement=what):
                self.assertTrue(what.strip(), "a measurement has no description")
                self.assertGreater(gib, 0)

    def test_an_existing_target_reduces_what_the_run_still_needs(self):
        """Otherwise the guard refuses runs that would have succeeded, which is its own false red.

        The integration gate runs on a warm `target/`. If the threshold ignored that, it would
        refuse to start with 20 GiB free on a tree that needed 3, and a guard that cries wolf is
        removed by the next person who is in a hurry.
        """
        cold = self.gate.required_free_bytes(0)
        warm = self.gate.required_free_bytes(self.gate.COLD_GATE_TARGET_BYTES)
        self.assertLess(warm, cold, "an already-built target/ bought the run nothing")
        self.assertGreater(warm, 0, "a warm target/ was treated as needing no disk at all")
        self.assertEqual(
            warm,
            self.gate.required_free_bytes(self.gate.COLD_GATE_TARGET_BYTES * 4),
            "a target/ larger than any measured cold build asked for less than the headroom",
        )

    # -- telling infrastructure from a red step ------------------------------------------------

    def test_cargos_misleading_failures_are_read_as_infrastructure(self):
        """The three real messages from the evening of 2026-07-29, verbatim from the story.

        Every one of them reads as a code error. All three were a `target/` that had vanished
        under cargo, in two cases because the device was full.
        """
        for line in (
            "error: failed to create file "
            "'/home/x/sipx/target/debug/examples/canned_program.d': "
            "No such file or directory (os error 2)",
            "error: failed to write "
            "'/home/x/sipx/target/debug/.fingerprint/rand-9a1/invoked.timestamp'",
            "error: extern location for autocfg does not exist: "
            "/home/x/sipx/target/debug/deps/libautocfg-4d1.rlib",
            "error: failed to write to file: No space left on device (os error 28)",
        ):
            with self.subTest(message=line[:60]):
                found = self.gate.infrastructure_evidence(line)
                self.assertIsNotNone(
                    found,
                    "cargo's message for a vanished build directory was accepted as a code "
                    "defect, which is exactly how a correct merge was nearly reverted",
                )
                evidence, why = found
                self.assertIn(evidence, line)
                self.assertTrue(why.strip(), "the classification does not say why it is not a diff")

    def test_a_real_compile_error_stays_a_red_step(self):
        """The guard must not swallow the failures the gate exists to find.

        Reading a genuine defect as infrastructure is the more dangerous direction: it would tell
        an implementor to re-run rather than to look, which is the disease and not the cure.
        """
        for line in (
            "error[E0308]: mismatched types",
            "  --> crates/sipx-sip/src/parser.rs:42:5",
            "error: couldn't read crates/sipx-sip/src/missing.rs: "
            "No such file or directory (os error 2)",
            "thread 'via::branch_is_rejected' panicked at crates/sipx-sip/src/via.rs:88:9",
            "test result: FAILED. 412 passed; 1 failed; 0 ignored",
        ):
            with self.subTest(message=line[:60]):
                self.assertIsNone(
                    self.gate.infrastructure_evidence(line),
                    "a real failure was excused as infrastructure, which trains everyone to "
                    "re-run the gate instead of reading it",
                )

    def test_an_infrastructure_failure_does_not_report_like_a_red_step(self):
        """The distinction is the whole story: "your diff is wrong" and "this proved nothing".

        A red step and an exhausted disk must not print the same way and must not exit the same
        way, because the cost being paid is humans reading the first when the truth was the second.
        """
        report = self.gate.infrastructure_report(
            step="test",
            evidence="error: failed to write '/home/x/sipx/target/debug/.fingerprint/rand-9a1/"
            "invoked.timestamp'",
            why="the path is inside target/",
            free=0,
            required=25 * self.GIB,
        )
        self.assertNotIn("steps failed", report, "an infrastructure failure printed as a red step")
        self.assertIn("proved nothing", report, "the report does not say the run means nothing")
        self.assertIn("disk", report.lower(), "the report does not name disk")
        self.assertIn("test", report, "the report does not name the step that died")
        self.assertNotEqual(
            self.gate.EXIT_RED,
            self.gate.EXIT_INFRASTRUCTURE,
            "a full disk and a broken diff leave the same exit code, so nothing scripted can "
            "tell a result from a non-result",
        )
        self.assertNotEqual(0, self.gate.EXIT_INFRASTRUCTURE, "a non-result exited green")

    # -- the decision the story asked for ------------------------------------------------------

    def test_the_shared_target_directory_decision_is_recorded(self):
        """`X-34` asked for a decision on a shared `CARGO_TARGET_DIR`, either way, written down.

        Asserted against the file because the point of recording it is that the next person to
        run out of disk finds the reasoning instead of re-deriving it.
        """
        source = (ROOT / "scripts" / "gate.py").read_text()
        self.assertIn(
            "CARGO_TARGET_DIR",
            source,
            "the gate does not record a decision on sharing one target directory between "
            "worktrees, so the next person out of disk has to re-derive it",
        )


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
