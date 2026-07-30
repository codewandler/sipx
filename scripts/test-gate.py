#!/usr/bin/env python3
"""Tests for the gate entry point, its drift check, its disk guard, the `docs site` step's own
contract and the fixed-sleep rule (stories `X-22`, `X-34`, `X-41`, `X-44`).

The gate is the contract for "before marking any story done", and for five days it was a list of
commands that omitted one CI runs. Every documented command passed, the `msrv` job was red, and
two releases shipped that did not build on the Rust version they advertise.

So the thing under test here is not "does the gate run". It is the ways a gate stops being worth
believing: a CI job the gate never learns about, an `AGENTS.md` gate block that grows its own copy
of the list and falls behind, `X-34`'s red result that was never about the tree at all, and
`X-41`'s green result that was — a step that printed a dead link in the published site and exited
0. Nearly all of it is asserted about real repository files rather than about fixtures, because a
check that only ever sees its own fixtures is the same class of mistake one level up.

`X-44` adds the fourth of those: a rule the project had written down normatively, swept for twice,
and never made executable — so two fresh violations of it landed in the wave after the sweep that
declared the workspace clean.
"""

import importlib.util
import pathlib
import subprocess
import sys
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parent.parent
AGENTS = ROOT / "AGENTS.md"
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"
BUILD_DOCS = ROOT / "scripts" / "build-docs.sh"
DOCS_LINKS = ROOT / "scripts" / "check-docs-links.py"
FIXED_SLEEP = ROOT / "scripts" / "check-fixed-sleep.py"
SITE_CONFIG = ROOT / "website" / "docusaurus.config.js"
# The dead-anchor probe `build-docs.sh` writes to prove the site's anchor guard is armed. Named
# here because three tests below are about the file rather than about the check.
PROBE_PREFIX = "website/docs/zz-anchor-guard-probe-"

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
        required = self.gate.REQUIRED_FREE_BYTES
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
        required = self.gate.REQUIRED_FREE_BYTES
        self.assertIsNone(self.gate.disk_problem(free=required, required=required))
        self.assertIsNone(self.gate.disk_problem(free=required * 4, required=required))

    # -- the threshold is measured -------------------------------------------------------------

    def test_the_threshold_covers_every_size_ever_measured(self):
        """Derived from measurements, not chosen — the story's words are "rather than guessed"."""
        self.assertTrue(self.gate.MEASURED_GATE_TARGET_GIB, "no measurement backs the threshold")
        largest = max(self.gate.MEASURED_GATE_TARGET_GIB.values())
        self.assertGreaterEqual(
            self.gate.REQUIRED_FREE_BYTES,
            largest * self.GIB,
            "the guard would let a run start with less disk than a run has actually been measured "
            "to consume, which is the guess this story exists to remove",
        )

    def test_every_measurement_says_where_it_came_from(self):
        """A number with no provenance is a guess that has been rounded."""
        for what, gib in self.gate.MEASURED_GATE_TARGET_GIB.items():
            with self.subTest(measurement=what):
                self.assertTrue(what.strip(), "a measurement has no description")
                self.assertGreater(gib, 0)

    def test_the_between_steps_floor_is_lower_than_the_starting_threshold(self):
        """A run already under way has paid for the steps it has run; it must not be re-charged.

        The floor exists for the other half of that evening: a disk another worktree fills while
        this gate is halfway through. Checking the full requirement again between steps would stop
        runs that were about to finish, and a guard that cries wolf is removed by the next person
        who is in a hurry.
        """
        self.assertLess(self.gate.FLOOR_FREE_BYTES, self.gate.REQUIRED_FREE_BYTES)
        self.assertGreater(self.gate.FLOOR_FREE_BYTES, 0, "the floor lets a run continue at 0 free")

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


class TheDocsSiteStep(unittest.TestCase):
    """`X-41`: the `docs site` step printed a dead link in the published site and exited 0.

    Docusaurus reports broken links, broken anchors, broken relative Markdown links and duplicate
    routes through four settings that default *independently*, and three of the four default to
    `warn` — print it, and exit 0. `onBrokenAnchors` was the one nobody had stated, so a link to
    `#cli-reference` (an id Docusaurus does not emit for a page's `h1`) produced

        [WARNING] Docusaurus found broken anchors! … [SUCCESS] Generated static files in "build".

    and the gate reported twenty-two steps green. `S-30` only found it by reading the step's
    output instead of its exit code.

    This class is the reversal detector for that. It is deliberately not the only one: the real
    end-to-end case — write a page linking to an anchor no page emits, and require the build to
    *fail* — lives in `build-docs.sh`, because the `gate` CI job has no node and a check that
    skips itself on the machine where it matters is the disease, not the cure. What is asserted
    here is the decision that check obeys, and that the check is still in the step at all.
    """

    HANDLERS = {
        "onBrokenLinks": "a link to a page the site does not publish",
        "onBrokenAnchors": "a link to an #anchor no page emits — the X-41 defect",
        "onDuplicateRoutes": "two routes claiming one path, so one page becomes unreachable",
        "onBrokenMarkdownLinks": "a relative Markdown link that resolves to no file",
    }

    def setUp(self):
        self.config = SITE_CONFIG.read_text(encoding="utf-8")
        self.script = BUILD_DOCS.read_text(encoding="utf-8")

    def handler(self, name: str):
        """The value the config states for a reporting handler, or `None` if it states none.

        Read out of the source rather than by importing the config: the config `require`s the
        search theme, so importing it needs `website/node_modules`, and this suite runs in a CI
        job that has no node at all.
        """
        import re

        found = re.search(rf"^\s*{name}:\s*'([a-z]+)'", self.config, re.MULTILINE)
        return found.group(1) if found else None

    # -- the four handlers, audited together ---------------------------------------------------

    def test_a_dead_anchor_is_a_build_failure_not_a_warning(self):
        """The defect, stated as a test. Fails on the commit this story starts from."""
        self.assertEqual(
            "throw",
            self.handler("onBrokenAnchors"),
            "onBrokenAnchors is not `throw` in website/docusaurus.config.js, so Docusaurus's "
            "default applies: a link to an anchor no page emits is printed and the build exits "
            "0. The `docs site` gate step is read by its exit code, so that is a green gate with "
            "a dead link in the published site (X-41)",
        )

    def test_every_reporting_handler_is_stated_rather_than_inherited(self):
        """`X-41`'s second half: fixing only anchors leaves three more defaults nobody read.

        Three of the four default to `warn`. A config that states one of them is a config whose
        reader cannot tell which of the other three were decided and which were forgotten.
        """
        for name, what in self.HANDLERS.items():
            with self.subTest(handler=name):
                value = self.handler(name)
                self.assertIsNotNone(
                    value,
                    f"{name} is not stated in website/docusaurus.config.js, so its Docusaurus "
                    f"default decides what happens to: {what}",
                )
                self.assertEqual(
                    "throw",
                    value,
                    f"{name} is `{value}`, so this is printed and the build exits 0: {what}",
                )

    def test_every_handler_carries_its_reason(self):
        """A value with no reason is the next person's judgement call, made without the history."""
        for name in self.HANDLERS:
            with self.subTest(handler=name):
                before = self.config.split(f"{name}:")[0]
                comment = [
                    line
                    for line in before.splitlines()[-8:]
                    if line.strip().startswith("//") and len(line.strip()) > 4
                ]
                self.assertTrue(
                    comment,
                    f"{name} is set with no comment above it saying what it catches and why it "
                    f"throws; a bare value is what gets relaxed by the next person in a hurry",
                )

    def test_the_markdown_link_handler_is_not_in_its_deprecated_place(self):
        """Docusaurus 3 moved it under `markdown.hooks`; the old spelling only warns about itself."""
        self.assertEqual(1, self.config.count("onBrokenMarkdownLinks:"))
        self.assertLess(
            self.config.index("hooks:"),
            self.config.index("onBrokenMarkdownLinks:"),
            "onBrokenMarkdownLinks is set at the top level, which Docusaurus 3 deprecates — it "
            "prints a deprecation warning and will stop being read in v4",
        )

    # -- the step cannot print a defect and exit 0 ---------------------------------------------

    def test_the_step_proves_its_anchor_guard_is_armed(self):
        """"The config says throw" and "a dead anchor fails this build" are two claims.

        The second is the one the gate rests on, so the step makes it itself: it writes a page
        linking to an id no page emits and requires that build to exit non-zero. Asserted here
        because deleting those lines is the cheap way to make this whole story evaporate.
        """
        self.assertIn(
            PROBE_PREFIX,
            self.script,
            "build-docs.sh no longer builds a page with a dead anchor to check that the guard "
            "is armed, so the step is back to trusting a config setting it never exercises",
        )
        self.assertIn(
            "BUILT SUCCESSFULLY",
            self.script,
            "build-docs.sh does not fail when the dead-anchor probe builds cleanly",
        )

    # -- and the probe cannot cost the next run anything ----------------------------------------
    #
    # The guard needs a page with a dead anchor, and Docusaurus only builds pages under
    # `website/docs`, so the probe lands in a tracked directory. A `kill -9` runs no `trap`, which
    # leaves two ways for a check about gate integrity to damage gate integrity: the leftover gets
    # committed, or it reddens every later run with no defect in the tree. Both are closed, and
    # both are asserted, because the closing is invisible from the diff that needs it.

    def test_a_leftover_probe_cannot_be_committed(self):
        """Asserted through `git check-ignore`, not by reading the pattern.

        The pattern is not the property. What matters is that `git add -A` — which is exactly what
        the integrating agent runs after a gate run — cannot stage a broken page into the published
        site.
        """
        probe = ROOT / f"{PROBE_PREFIX}31337.md"
        done = subprocess.run(
            ["git", "check-ignore", "-v", str(probe.relative_to(ROOT))],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(
            0,
            done.returncode,
            f"nothing ignores {probe.relative_to(ROOT)}, so a probe left behind by a killed gate "
            f"run is committable, and `git add -A` would put a page with a dead anchor into the "
            f"site. git said: {done.stdout or done.stderr!r}",
        )

    def test_the_stale_probe_sweep_runs_before_the_site_build(self):
        """Order is the whole fix, and it is invisible unless something checks it.

        A leftover probe makes the *real* site build fail — correctly, it is a dead anchor in the
        tree. Under `set -e` that aborts the script, so a cleanup placed next to the probe never
        runs and the gate stays red on every later run until a human deletes a gitignored file
        they cannot see in `git status`. Sweeping first is what makes a killed run cost nothing.
        """
        sweep = self.script.index("rm -f $PROBE_GLOB")
        build = self.script.index("npm run build 2>&1 | tee")
        self.assertLess(
            sweep,
            build,
            "the stale-probe sweep is after the site build, so a probe left by a killed run "
            "reddens the gate forever: the build aborts on it before the cleanup is reached",
        )

    def test_concurrent_runs_in_one_checkout_do_not_share_a_probe_path(self):
        """One run's cleanup must not delete another run's live probe and fake a "not armed" red."""
        self.assertIn(
            'PROBE="website/docs/zz-anchor-guard-probe-$$.md"',
            self.script,
            "the probe path is fixed, so two runs in one checkout race on it",
        )
        self.assertIn(
            'rm -f "$PROBE"',
            self.script,
            "the end-of-guard removal uses the glob rather than this run's own path, which would "
            "delete a concurrent run's live probe",
        )

    def test_the_step_treats_a_site_build_warning_as_a_defect(self):
        """The general form of the defect, for the handler Docusaurus adds next.

        Four handlers are audited above. A fifth in some later version would print and exit 0
        under a heading nobody here has read yet, so the step fails on any `[WARNING]` from the
        site build, with a named list for the exceptions rather than a deleted check.
        """
        self.assertIn("[WARNING]", self.script)
        self.assertIn("WARNING_EXCEPTIONS", self.script)

    def test_the_exceptions_list_is_empty(self):
        """Every entry would be a defect the step chooses to print and exit 0 about."""
        self.assertIn(
            "WARNING_EXCEPTIONS=()",
            self.script,
            "the site build has warnings it is excusing; each one needs a reason on the line and "
            "a story against it, because each one is X-41 again",
        )

    def test_the_step_cannot_lose_an_exit_code_down_a_pipe(self):
        """It pipes the build into a log to read it. Without `pipefail` that discards the result."""
        self.assertIn("set -euo pipefail", self.script)
        self.assertIn("| tee", self.script, "the step no longer captures the site build's output")


class TheInternalDocsLinkCheck(unittest.TestCase):
    """`check-docs-links.py`: the unpublished half of the docs, checked to the same depth.

    Docusaurus never sees `docs/`, so its link graph is checked by this script or by nothing. Its
    predecessor — a heredoc inside `build-docs.sh` — split every link on `#` and threw the
    fragment away, so a link to a missing *file* failed the build and a link to a missing
    *heading* was invisible. That is `X-41` one directory over, and it is why the fixtures below
    are about anchors.
    """

    @classmethod
    def setUpClass(cls):
        sys.dont_write_bytecode = True
        spec = importlib.util.spec_from_file_location("check_docs_links", DOCS_LINKS)
        cls.mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.mod)

    def tree(self, pages: dict) -> pathlib.Path:
        """A throwaway docs tree, so a fixture cannot be satisfied by the real repository."""
        import tempfile

        root = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(__import__("shutil").rmtree, root, True)
        for name, text in pages.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        (root / "docs").mkdir(exist_ok=True)
        return root

    def problems(self, pages: dict) -> list:
        return self.mod.check(self.tree(pages)).problems

    # -- the repository itself -----------------------------------------------------------------

    def test_the_repository_has_no_dead_link_or_anchor(self):
        report = self.mod.check(ROOT)
        self.assertEqual([], report.problems)
        self.assertGreater(report.links, 0, "the checker found no links, so it proves nothing")
        self.assertGreater(report.anchor_links, 0, "the checker found no anchors to check")

    # -- anchors ------------------------------------------------------------------------------

    def test_a_link_to_a_nonexistent_anchor_is_reported(self):
        """The failing-first assertion for the `docs/` half of X-41."""
        problems = self.problems(
            {
                "docs/a.md": "# A\n\nSee [b](b.md#no-such-heading).\n",
                "docs/b.md": "# B\n\n## Real heading\n",
            }
        )
        self.assertTrue(
            any("no-such-heading" in p for p in problems),
            f"a link to a heading that does not exist was accepted; problems={problems}",
        )

    def test_a_link_to_a_real_anchor_is_accepted(self):
        """The guard must cost a correct link nothing, or it gets deleted rather than fixed."""
        self.assertEqual(
            [],
            self.problems(
                {
                    "docs/a.md": "# A\n\nSee [b](b.md#real-heading) and [self](#a).\n",
                    "docs/b.md": "# B\n\n## Real heading\n",
                }
            ),
        )

    def test_a_dropped_character_between_two_spaces_leaves_two_hyphens(self):
        """The rule that made two live links in `docs/roadmap.md` look dead while this was written.

        `### Application SDK — `app-sdk`` anchors as `application-sdk--app-sdk`: the em dash is
        removed and both spaces around it survive as hyphens. A slugger that collapses runs of
        whitespace reports the real link as broken, and the next person "fixes" a correct link.
        """
        self.assertEqual(
            [],
            self.problems(
                {
                    "docs/a.md": "# A\n\n[x](b.md#application-sdk--app-sdk)\n",
                    "docs/b.md": "# B\n\n### Application SDK — `app-sdk`\n",
                }
            ),
        )

    def test_underscore_emphasis_is_consumed_like_any_other_markup(self):
        """`_(six stories, M10)_` contributes `six-stories-m10`, not `_six-stories-m10_`.

        `_` survives the punctuation strip because it is a word character, so leaving emphasis
        underscores in changes the id. Three headings in `docs/roadmap.md` have this shape.
        """
        self.assertEqual(
            [],
            self.problems(
                {
                    "docs/a.md": "# A\n\n[x](b.md#ice--ice-six-stories-m10)\n",
                    "docs/b.md": "# B\n\n### ICE — `ice` _(six stories, M10)_\n",
                }
            ),
        )

    def test_an_underscore_inside_a_word_is_not_emphasis(self):
        """The other direction, and the reason the rule is flanked rather than greedy.

        `on_failure` and `snake_case` are identifiers. Consuming their underscores would break
        every anchor that names one, which is most of what the specs' headings are.
        """
        self.assertEqual(
            [],
            self.problems(
                {
                    "docs/a.md": "# A\n\n[x](b.md#the-on_failure-table)\n",
                    "docs/b.md": "# B\n\n## The on_failure table\n",
                }
            ),
        )

    def test_markup_inside_a_code_span_is_not_markup(self):
        """``[listener.<name>]`` in a code span is characters, not an HTML tag.

        This slugged as `42-listener` while it was being written, because backticks were stripped
        before tags were and `<name>` was eaten as a tag. Four headings in
        `docs/specs/host-config.md` have the shape. Code spans are held out of every markup rule
        now, so the answer cannot depend on the order those rules happen to run in.
        """
        self.assertEqual(
            [],
            self.problems(
                {
                    "docs/a.md": "# A\n\n[x](b.md#42-listenername) [y](b.md#45-appnameon_failure-n4)\n",
                    "docs/b.md": "# B\n\n### 4.2 `[listener.<name>]`\n\n"
                    "### 4.5 `[app.<name>.on_failure]` (N4)\n",
                }
            ),
        )

    def test_the_headings_in_the_tree_with_these_shapes_slug_as_the_renderer_does(self):
        """The seven real headings the shapes above were found in, held against the real files.

        A fixture proves the rule; these prove the rule is about this repository. None of them is
        linked today, so none of them can redden the gate yet — which is exactly why they would
        otherwise go on diverging unnoticed until someone links one.
        """
        expected = {
            # `X-50` removed this heading's `_(six stories, M10)_` parenthetical, because it was a
            # second statement of M10's exit criterion. Re-derived rather than deleted, per the
            # assertion below: the em-dash-plus-backticks shape is what this case is here for, and
            # the two entries under it still cover the parenthetical half.
            "ICE — `ice`": "ice--ice",
            "Endpoint discovery — `discovery` _(four stories)_": (
                "endpoint-discovery--discovery-four-stories"
            ),
            "Edge / B2BUA — `edge` _(one story, in M9)_": "edge--b2bua--edge-one-story-in-m9",
            "4.2 `[listener.<name>]`": "42-listenername",
            "4.3 `[app.<name>]`": "43-appname",
            "4.5 `[app.<name>.on_failure]` (N4)": "45-appnameon_failure-n4",
            "4.6 `[app.<name>.grants]` (N5)": "46-appnamegrants-n5",
        }
        headings = set()
        for name in ("docs/roadmap.md", "docs/specs/host-config.md"):
            for line in (ROOT / name).read_text(encoding="utf-8").splitlines():
                if line.startswith("#"):
                    headings.add(line.lstrip("#").strip())
        for heading, anchor in expected.items():
            with self.subTest(heading=heading):
                self.assertIn(
                    heading,
                    headings,
                    "this heading has been reworded; re-derive its anchor rather than deleting "
                    "the case, or the shape goes back to being unchecked",
                )
                self.assertEqual(anchor, self.mod.slug(heading))

    def test_an_explicit_id_wins_over_the_heading_text(self):
        self.assertEqual(
            [],
            self.problems(
                {
                    "docs/a.md": "# A\n\n[x](b.md#chosen) and [y](b.md#anchored)\n",
                    "docs/b.md": '# B\n\n## Some heading {#chosen}\n\n<a name="anchored"></a>\n',
                }
            ),
        )

    def test_a_repeated_heading_gets_the_numbered_form(self):
        problems = self.problems(
            {
                "docs/a.md": "# A\n\n[first](b.md#notes) [second](b.md#notes-1) [third](b.md#notes-2)\n",
                "docs/b.md": "# B\n\n## Notes\n\n## Notes\n",
            }
        )
        self.assertTrue(
            any("notes-2" in p for p in problems),
            f"a third `## Notes` that does not exist was accepted; problems={problems}",
        )
        self.assertFalse(
            any("notes-1" in p for p in problems),
            f"the second `## Notes` was not given its `-1` form; problems={problems}",
        )

    def test_a_fragment_on_something_that_is_not_markdown_is_left_alone(self):
        """`file.rs#L42` is a line range. The file's existence is ours; the fragment is not."""
        self.assertEqual(
            [],
            self.problems({"docs/a.md": "# A\n\n[code](b.rs#L42)\n", "docs/b.rs": "fn main() {}\n"}),
        )

    def test_a_link_inside_a_fenced_block_is_not_a_link(self):
        """`docs/` is full of sample markdown; its illustrations are not navigation."""
        self.assertEqual(
            [],
            self.problems(
                {"docs/a.md": "# A\n\n```md\nSee [nothing](gone.md#nowhere).\n```\n"}
            ),
        )

    # -- the file half, which must not regress -------------------------------------------------

    def test_a_link_to_a_missing_file_is_still_reported(self):
        problems = self.problems({"docs/a.md": "# A\n\n[gone](gone.md)\n"})
        self.assertTrue(
            any("gone.md" in p for p in problems),
            f"the check this script inherited was lost in the move; problems={problems}",
        )

    def test_the_script_exits_non_zero_on_a_problem(self):
        """The exit code is what the gate step reads, so the exit code is asserted."""
        root = self.tree({"docs/a.md": "# A\n\n[x](b.md#nope)\n", "docs/b.md": "# B\n"})
        done = subprocess.run(
            [sys.executable, str(DOCS_LINKS), "--root", str(root)],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(0, done.returncode, "a dead anchor was printed and exited 0")
        self.assertIn("nope", done.stderr + done.stdout)


class TheFixedSleepRule(unittest.TestCase):
    """`X-44`: `docs/designs/media.md`'s rule, made executable.

    The rule is normative and old: *a fixed wall-clock duration may bound a failure, or define
    silence. It may not stand in for a happens-before.* `X-28` cleared the media path of
    violations, `X-29` swept the rest of the workspace, and `0.12.0`'s changelog says "no test in
    the workspace now asserts after a fixed sleep". Nothing enforced any of it, so the sentence was
    true on the day it was written and by nothing afterwards — and two violations landed inside the
    very next wave, both caught by a human reading a diff.

    What is asserted here is the property that makes the guard worth having rather than the
    property that makes it look like it works:

    * a planted violation is refused, in a fixture tree, at a path that looks exactly like a real
      one — so nothing can be passing because of where it sits;
    * **a rename cannot get past it.** `sleep` is one spelling of a wall-clock wait and the guard
      is not allowed to be a grep for it;
    * the legitimate categories cost nothing, because a guard that reddens correct code is deleted
      rather than fixed;
    * and the repository itself is clean, which is the claim `0.12.0` made and could not keep.
    """

    #: The shape of the defect: something is stimulated, a fixed duration passes, and the test
    #: asserts the stimulus arrived. What load does to it is fail the assertion, which is why this
    #: family is re-run rather than read.
    VIOLATION = """\
#[tokio::test]
async fn a_packet_is_forwarded() {
    let peer = start().await;
    peer.send(&packet()).await;
    %s
    assert_eq!(peer.received(), 1, "the packet must have been forwarded");
}
"""

    #: Six ways to write the same wait. The guard has to see the shape, not the word — a check that
    #: greps `sleep(` invites the next author to write the identical defect in the next row down,
    #: which is the "rule fitted to the data it was tested on" failure this project keeps warning
    #: about.
    SPELLINGS = {
        "tokio::time::sleep": "tokio::time::sleep(Duration::from_millis(150)).await;",
        "std::thread::sleep": "std::thread::sleep(Duration::from_millis(150));",
        "sleep_until": (
            "tokio::time::sleep_until(tokio::time::Instant::now() "
            "+ Duration::from_millis(150)).await;"
        ),
        "an interval tick": "interval.tick().await;",
        "a hand-rolled deadline loop": (
            "let deadline = std::time::Instant::now() + Duration::from_millis(150);\n"
            "    while std::time::Instant::now() < deadline {}"
        ),
        # A private helper that wraps a bare wait is a rename with an extra step, and the one a
        # grep for `sleep` is guaranteed to miss.
        "a locally renamed wrapper": "settle_for_a_moment().await;",
    }

    #: The helper the last spelling above hides behind, appended to that fixture.
    WRAPPER = """
async fn settle_for_a_moment() {
    tokio::time::sleep(Duration::from_millis(150)).await;
}
"""

    def setUp(self):
        """Per test rather than per class, so each case below reports its own absence.

        A `setUpClass` that raised would collapse eight distinct claims into one line, and the
        first thing anybody does with this suite is read which of them is red.
        """
        self.assertTrue(
            FIXED_SLEEP.exists(),
            f"{FIXED_SLEEP.relative_to(ROOT)} does not exist, so nothing in this repository "
            f"enforces docs/designs/media.md's rule — a fixed wall-clock duration may bound a "
            f"failure or define silence, and may not stand in for a happens-before. The rule has "
            f"been swept for twice and held by nobody since (X-44)",
        )
        sys.dont_write_bytecode = True
        spec = importlib.util.spec_from_file_location("check_fixed_sleep", FIXED_SLEEP)
        self.mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.mod)

    def tree(self, files: dict) -> pathlib.Path:
        """A throwaway workspace, so a fixture cannot be satisfied by the real repository."""
        import tempfile

        root = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(__import__("shutil").rmtree, root, True)
        for name, text in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        return root

    def problems(self, body: str, where: str = "crates/sipx-call/tests/call.rs") -> list:
        return self.mod.check(self.tree({where: body})).problems

    # -- the defect ----------------------------------------------------------------------------

    def test_a_fixed_sleep_before_an_assertion_is_refused(self):
        """The failing-first assertion, and the shape of both regressions this story was filed for.

        Planted at a path that is a real file in this repository, so a guard that passed by knowing
        which files are allowed to sleep would fail here.
        """
        problems = self.problems(self.VIOLATION % self.SPELLINGS["tokio::time::sleep"])
        self.assertTrue(
            problems,
            "a test that sleeps for a fixed duration and then asserts the thing it was waiting "
            "for arrived was accepted; that is the defect docs/designs/media.md forbids, and it "
            "is what `0.12.0` claimed the workspace no longer contained",
        )

    def test_a_rename_does_not_get_past_it(self):
        """The guard identifies the wait by its shape. Every spelling is the same defect."""
        for name, spelling in self.SPELLINGS.items():
            with self.subTest(spelling=name):
                body = self.VIOLATION % spelling
                if "settle_for_a_moment" in spelling:
                    body += self.WRAPPER
                self.assertTrue(
                    self.problems(body),
                    f"the same defect written as `{name}` was accepted, so the guard is a grep "
                    f"for one spelling and the next author only has to write it differently",
                )

    def test_the_rule_covers_production_code_as_well_as_tests(self):
        """`X-40` proved the defect can live in `src/`, and most of this workspace's tests do too.

        `crates/sipx-media/src/session.rs` holds more fixed-duration waits than any file under a
        `tests/` directory, because the media suite lives in `#[cfg(test)]` modules beside the code
        it tests. A guard that read `tests/` would have covered less than half of the suite it is
        for, and none of the production code `X-40`'s defect was written in.
        """
        self.assertTrue(
            self.problems(
                self.VIOLATION % self.SPELLINGS["tokio::time::sleep"],
                where="crates/sipx-media/src/session.rs",
            ),
            "a fixed-sleep assertion inside `src/` was accepted; the rule would not have covered "
            "`record_until_idle`, which is where X-40's instance of it was written",
        )

    def test_a_reason_that_classifies_nothing_is_still_refused(self):
        """Prose is not the requirement — a *classification* is.

        The rule names the questions a fixed duration is allowed to answer. A comment that says
        the author waited, without saying which question the duration answers, leaves the next
        reader exactly where the unannotated version did.
        """
        body = self.VIOLATION % (
            "// Give the far end a moment to catch up.\n"
            "    tokio::time::sleep(Duration::from_millis(150)).await;"
        )
        self.assertTrue(
            self.problems(body),
            "a comment that describes the wait rather than classifying the duration was accepted "
            "as the reason at the call site",
        )

    # -- the categories that are allowed to stay -----------------------------------------------

    def test_a_bound_on_failure_is_not_refused(self):
        """`record_at_least`'s `within`, `DELIVERY_BOUND`, `collect_digits`' `within`.

        The duration is how long this side waits before concluding the thing is not coming. It is
        checked structurally as well as by name: the completion condition is the event, not the
        clock, so the wait is not a bare one at all.
        """
        self.assertEqual(
            [],
            self.problems(
                """\
#[tokio::test]
async fn a_packet_is_forwarded() {
    let peer = start().await;
    peer.send(&packet()).await;
    // A bound on failure: how long before we conclude the packet is not coming.
    let got = tokio::time::timeout(DELIVERY_BOUND, peer.recv()).await;
    assert!(got.is_ok(), "the packet must have been forwarded");
}
"""
            ),
        )

    def test_a_definition_of_silence_is_not_refused(self):
        """The idle window: how long a hole has to be to mean the far end stopped.

        `X-40` is the caveat, not the exception — an idle window must not also be a start
        deadline. What the site has to say is which of the two it is.
        """
        self.assertEqual(
            [],
            self.problems(
                """\
#[tokio::test]
async fn a_stopped_session_sends_nothing() {
    let peer = start().await;
    let before = peer.sent();
    // A definition of silence: how long a hole has to be before "it stopped" is true. The
    // assertion is negative, so load lengthens the window and can only make it fail.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(peer.sent(), before, "a stopped session sends nothing");
}
"""
            ),
        )

    def test_a_clock_that_is_the_measurement_is_not_refused(self):
        """`X-29`'s third category, and `crates/sipx-cli/tests/cli.rs`'s `elapsed() < 12s`.

        The assertion is about *which* duration elapsed — 3 s or 64*T1's 32 s — so the clock is
        not standing in for anything. Load can only push the number up, which is the direction
        that fails.
        """
        self.assertEqual(
            [],
            self.problems(
                """\
#[tokio::test]
async fn it_gives_up_on_its_own_schedule() {
    let started = std::time::Instant::now();
    let output = run_the_dial().await;
    assert_eq!(output.code(), Some(5));
    // The clock is the measurement: the whole claim is which of two schedules fired, and the
    // only way to read that is the clock. 12 s separates our 3 s from 64*T1's 32 s.
    assert!(started.elapsed() < Duration::from_secs(12), "{:?}", started.elapsed());
}
"""
            ),
        )

    def test_a_causal_wait_costs_a_correct_test_nothing(self):
        """Neither shape needs a word written about it, because neither waits on a clock.

        A guard that made the correct thing more expensive than the wrong one would be switched
        off by whoever hit it second. Waiting for the event, and polling until the condition
        holds, are the two fixes the rule asks for — so both are recognised by their shape.
        """
        self.assertEqual(
            [],
            self.problems(
                """\
async fn until(bound: Duration, what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + bound;
    loop {
        if ready() {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn a_packet_is_forwarded() {
    let peer = start().await;
    peer.send(&packet()).await;
    until(SIGNALLING_BOUND, "the packet never arrived", || peer.received() == 1).await;
    assert_eq!(peer.received(), 1);
}
"""
            ),
        )

    #: `X-40`'s defect, as it stood in `crates/sipx-cli/tests/interop_media/mod.rs` before the fix:
    #: one 600 ms window answering both "how long may the echo take to start" and "how long a gap
    #: means it has ended". A late first packet left the payload empty, and the harness reported
    #: "no audio came back" on a call that carried it.
    ONE_WINDOW_TWO_QUESTIONS = """\
async fn echo_round_trip(media: &Media) -> Echoed {
    let mut echoed = Echoed::new();
    // Stop when the far end goes quiet, not after a fixed count.
    while let Ok(Some(packet)) =
        tokio::time::timeout(Duration::from_millis(600), media.recv_encoded()).await
    {
        echoed.push(packet);
    }
    echoed
}
"""

    def test_one_duration_answering_two_questions_is_refused(self):
        """The other regression the story was filed for, and the one a sleep-grep cannot see.

        There is no `sleep` here at all. What makes it the same defect is that a *relative* timeout
        restarts on every pass, so the first pass's window is a start deadline and every later one
        is an inter-arrival gap — two questions, one duration, and whichever is tighter on the day
        loses. The result is an empty collection rather than a short one, because the loop ends
        before its first iteration.
        """
        self.assertTrue(
            self.problems(
                self.ONE_WINDOW_TWO_QUESTIONS, where="crates/sipx-cli/tests/interop_media/mod.rs"
            ),
            "a loop spending one window on `has it started` and `has it ended` was accepted — "
            "that is X-40 exactly, and it shipped",
        )

    def test_the_fix_for_it_costs_nothing(self):
        """Both halves of `X-40`'s remedy, recognised structurally rather than apologised for.

        A variable the body reassigns after the first arrival is two durations spelled
        economically; an absolute `timeout_at` bounds the whole loop once, which is a bound on
        failure and what `record_at_least` was written to be. A guard that charged for its own
        remedy would be the reason the next author does not apply it.
        """
        reassigned = """\
async fn echo_round_trip(media: &Media) -> Echoed {
    let mut echoed = Echoed::new();
    let mut window = Duration::from_secs(10);
    while let Ok(Some(packet)) = tokio::time::timeout(window, media.recv_encoded()).await {
        echoed.push(packet);
        window = Duration::from_millis(600);
    }
    echoed
}
"""
        absolute = """\
async fn record_at_least(media: &Media, samples: usize, within: Duration) -> Vec<i16> {
    let deadline = tokio::time::Instant::now() + within;
    let mut recorded = Vec::new();
    while recorded.len() < samples {
        match tokio::time::timeout_at(deadline, media.recv()).await {
            Ok(Some(frame)) => recorded.extend_from_slice(&frame),
            _ => break,
        }
    }
    recorded
}
"""
        for name, body in (("a reassigned window", reassigned), ("an absolute deadline", absolute)):
            with self.subTest(fix=name):
                self.assertEqual([], self.problems(body))

    def test_a_deadline_loop_that_waits_on_an_event_is_a_poll_and_not_a_spin(self):
        """The same head opens the defect and its fix, so the body is what decides.

        `while now < deadline { … }` is a spin when it awaits nothing and leaves on nothing, and a
        poll with a bound on failure when it awaits the event and breaks on it —
        `a_cancel_waits_for_a_late_provisional_rather_than_being_abandoned` is the second, and
        reporting it would have asked an author to apologise for writing the fix.
        """
        self.assertEqual(
            [],
            self.problems(
                """\
#[tokio::test]
async fn the_cancel_follows_the_provisional() {
    let peer = start().await;
    let mut cancelled = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, peer.recv()).await {
            Ok(Ok(message)) => {
                if message.starts_with("CANCEL ") {
                    cancelled = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(cancelled, "the CANCEL must follow the provisional it was waiting for");
}
"""
            ),
        )

    def test_a_signature_with_no_body_does_not_swallow_the_rest_of_the_file(self):
        """A reader that loses its place reports nothing, which looks exactly like a clean tree.

        A trait's `fn ready(&self) -> bool;` opens no block. Treating it as a function stretching to
        the end of the file would put every later function inside it, and the violation below —
        which is the whole point — would be read as belonging to a span that never ends.
        """
        self.assertTrue(
            self.problems(
                """\
trait Peer {
    fn ready(&self) -> bool;
    fn received(&self) -> usize;
}

"""
                + self.VIOLATION % self.SPELLINGS["tokio::time::sleep"]
            ),
            "a violation after a body-less signature was missed, so the reader had lost its place",
        )

    def test_a_paused_clock_is_not_a_wall_clock(self):
        """`#[tokio::test(start_paused = true)]` has no wall clock to race.

        Time only moves when the runtime is asked to move it, so `advance` is deterministic and
        the whole family of failures this rule is about cannot occur. Reporting it would teach
        implementors that the honest fix — a virtual clock — costs them a comment.
        """
        self.assertEqual(
            [],
            self.problems(
                """\
#[tokio::test(start_paused = true)]
async fn the_session_expires() {
    let call = connected().await;
    tokio::time::advance(Duration::from_secs(61)).await;
    assert!(call.is_ended(), "the session timer must have fired");
}
"""
            ),
        )

    # -- no suppression list, under any name ---------------------------------------------------

    def test_the_guard_knows_no_path(self):
        """`X-35`'s standard, applied here: the reason lives at the call site or nowhere.

        Asserted by planting the same violation under four different real-looking paths and
        requiring all four to be refused. A list of blessed files is the thing that makes a guard
        stop being a guard, and it is usually added one entry at a time by someone in a hurry.
        """
        for where in (
            "crates/sipx-call/tests/call.rs",
            "crates/sipx-cli/tests/interop_media/mod.rs",
            "crates/sipx-media/src/session.rs",
            "crates/sipx-testkit/src/soak.rs",
        ):
            with self.subTest(path=where):
                self.assertTrue(
                    self.problems(
                        self.VIOLATION % self.SPELLINGS["tokio::time::sleep"], where=where
                    ),
                    f"the violation was accepted at {where}, so some path is exempt",
                )

    def test_every_category_states_what_it_means(self):
        """A marker with no explanation is a password, and passwords get typed without thinking."""
        self.assertTrue(self.mod.CATEGORIES, "the guard recognises no categories at all")
        for category in self.mod.CATEGORIES.values():
            with self.subTest(category=category.name):
                self.assertTrue(category.written, f"{category.name} has no spelling to look for")
                self.assertTrue(category.means.strip(), f"{category.name} says nothing about itself")

    # -- the repository itself -----------------------------------------------------------------

    def test_the_repository_itself_has_no_unclassified_fixed_wait(self):
        """The claim `0.12.0` made about the workspace, asserted instead of asserted-to.

        This is the case that fails on the commit this story starts from, and it is the reason the
        guard exists rather than a third sweep: the property has to be checked by something that
        runs, not by whoever last looked.
        """
        report = self.mod.check(ROOT)
        self.assertEqual([], report.problems)
        self.assertGreater(
            report.waits, 0, "the guard found no wall-clock wait anywhere, so it proves nothing"
        )
        self.assertGreater(
            report.classified,
            0,
            "the guard found no classified duration, so the reasons it demands are not being read",
        )

    def test_the_script_exits_non_zero_on_a_problem(self):
        """The exit code is what the gate step reads, so the exit code is asserted."""
        root = self.tree(
            {
                "crates/sipx-call/tests/call.rs": self.VIOLATION
                % self.SPELLINGS["tokio::time::sleep"]
            }
        )
        done = subprocess.run(
            [sys.executable, str(FIXED_SLEEP), "--check", "--root", str(root)],
            capture_output=True,
            text=True,
        )
        self.assertNotEqual(0, done.returncode, "a fixed-sleep assertion was printed and exited 0")
        self.assertIn("call.rs", done.stderr + done.stdout)

    def test_the_gate_runs_it_and_ci_does_too(self):
        """A check nothing runs is a file. `X-22`'s property, for this step."""
        steps = gate().gate_steps("1.0.0")
        mine = [step for step in steps if "check-fixed-sleep.py" in " ".join(step.command)]
        self.assertEqual(
            1, len(mine), "the fixed-sleep guard is not a gate step, so nothing runs it locally"
        )
        jobs = gate().parse_workflow(WORKFLOW.read_text())
        self.assertIn(
            mine[0].ci_job,
            jobs,
            f"gate step `{mine[0].name}` names CI job `{mine[0].ci_job}`, which ci.yml does not "
            f"define",
        )


if __name__ == "__main__":
    sys.exit(0 if unittest.main(exit=False, verbosity=2).result.wasSuccessful() else 1)
