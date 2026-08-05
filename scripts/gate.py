#!/usr/bin/env python3
"""Run the gate — the set of checks CI runs — and prove it is still the same set.

`AGENTS.md` calls the gate the contract for "before marking any story done", and until this
script it was a list of commands in a markdown file. A list has to be transcribed correctly, and
once it was not: CI has always run an `msrv` job that the list never named, so an implementor
could run every documented command, see green, and tag a release that does not build on the Rust
version it advertises. That is what happened — the `msrv` job was red from v0.4.0 through v0.7.0
and nobody was told, because nothing anyone ran locally covered it.

So the gate is now a program, and `--check` is the part that matters:

* Every command a CI job runs is either mirrored by a step here or named in `NOT_RUN_LOCALLY`
  with a reason. A new job fails the check until somebody decides which it is.
* A step that runs its job's command with fewer flags is drift too. `cargo check` without
  `--all-targets` passes on a tree whose tests do not compile on the MSRV, which is a green gate
  and a red CI one argument down. Real differences are declared per step, with why.
* `AGENTS.md` may invoke this script and say nothing else. A gate block that lists the commands
  is a second copy, and the second copy is the one that fell behind.

Nothing here is transcribed from anywhere. The MSRV toolchain comes from the workspace
`rust-version`, the environment from `ci.yml`'s own `env:` block, and the job list from `ci.yml`.
The only things written down twice are checked for equality.

The disk guard (X-34) is that same principle one layer over. `--check` refuses to report when the
gate no longer matches CI; the guard refuses to report when the machine cannot hold the build. A
gate that cannot be believed should not report — see "The disk guard" below for what it costs when
it does.

**Why a script** (X-22's recorded decision): a `just` or `make` target would be a second list in
a second syntax with no way to read `Cargo.toml` or `ci.yml`, and a cargo alias cannot run
anything that is not cargo — the gate is half shell scripts. A Python entry point keeps the step
list, the drift check and the version derivation in one file, which is the property that makes
the drift check worth having: there is nowhere for a step to exist unchecked.
"""

import argparse
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tomllib
from collections import defaultdict
from dataclasses import dataclass, field
from typing import NamedTuple

ROOT = pathlib.Path(__file__).resolve().parent.parent
SELF = pathlib.Path(__file__).resolve()
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"
MANIFEST = ROOT / "Cargo.toml"
AGENTS = ROOT / "AGENTS.md"

#: How `AGENTS.md` is allowed to spell the gate. Anything else in its gate block is a copy of the
#: step list, which is the failure this script exists to remove.
ENTRY_POINT = "./scripts/gate.py"


class Step(NamedTuple):
    """One check, and the CI job that is its counterpart."""

    #: What to call it in the summary.
    name: str
    #: The job in `ci.yml` that runs the same thing. Checked to exist, and to run this command.
    ci_job: str
    command: tuple[str, ...]
    #: Flags this step and its CI job deliberately differ on, each with the reason. A difference
    #: that is not declared here is drift, because the interesting ones look exactly like this.
    differs: tuple[tuple[str, str], ...] = ()
    #: A rustup toolchain that must be installed before the step can run. Set for the MSRV step:
    #: its absence has to be a failure, since a skipped MSRV check is indistinguishable from a
    #: passing one and that is precisely how this went unnoticed for five days.
    toolchain: str = ""
    #: Why this step is allowed to disclaim its own run by exiting `STEP_NOT_A_RESULT`, or `""`
    #: if it is not — the sentence the summary prints in place of a finding. Opt-in and per step
    #: (`X-58`), because the disclaimer only means anything from a script this repository owns and
    #: whose exit codes it therefore controls: a step whose command is `cargo` or `npm` exits what
    #: it exits, and reading a number out of a third-party tool as "ignore this" is how a real
    #: failure gets excused.
    not_a_result: str = ""


#: CI jobs that are deliberately not part of the local gate. Every job in `ci.yml` is either a
#: step above or an entry here — the check enforces that, so adding a job forces the decision.
NOT_RUN_LOCALLY = {
    "interop-peers": "reads the peer list for the matrix below; it verifies nothing itself",
    "interop": "needs a running container per peer; `tests/interop/run.sh --peer <name>` on demand",
    "soak": "minutes per run, which is why it is scheduled nightly rather than run per push",
    "fuzz": "a nightly toolchain and a cargo-fuzz install, for a run that is time-boxed anyway",
    "deny": "runs as a packaged action against a freshly fetched advisory database, not local state",
    "deploy-site": "publishes what `site` built; there is nothing to verify",
    "device-linux": "the local all-feature suite runs the x86 vector; CI adds the arm64 release architecture",
    "device-portable": "requires the macOS and Windows platform audio SDKs unavailable on a Linux gate host",
    "browser-audio": "requires the hosted runner's matched native browser/WebDriver; the local gate runs its adversarial harness suite",
}

#: Run commands that are runner provisioning rather than checks. Kept deliberately short: every
#: prefix here is a command the drift check stops looking at.
IGNORED_RUN_PREFIXES = ("sudo",)


def gate_steps(msrv: str) -> list[Step]:
    """The gate, cheapest first, so a typo in a script is not found after twenty minutes of cargo."""
    return [
        Step("gate consistency", "gate", (ENTRY_POINT, "--check")),
        Step("gate tests", "gate", ("python3", "scripts/test-gate.py")),
        # X-15 wrote a schema guard for the RFC registry and a suite for it, and nothing ran the
        # suite. It belongs here: it is a test of a gate script, it takes milliseconds, and the
        # alternative is a test file that can rot without anybody hearing about it.
        Step("rfc report tests", "gate", ("python3", "scripts/test-rfc-report.py")),
        Step("pool key tests", "gate", ("python3", "scripts/test-pool-key.py")),
        Step("audio claims tests", "gate", ("python3", "scripts/test-audio-claims.py")),
        # X-32: the maturity report is generated from the registry and the board, so its arithmetic
        # is the kind of thing that can be quietly wrong for a long time. Milliseconds, and it also
        # asserts that every predicate names a story that exists.
        Step("maturity tests", "gate", ("python3", "scripts/test-maturity.py")),
        # A-11: registry writes are deliberately absent from the gate. The release helper's
        # adversarial fixtures prove its ordering and authority boundary without credentials or a
        # registry connection; the exact clean release checkout runs the separate dry-run mode.
        Step("release rehearsal tests", "gate", ("python3", "scripts/test-release.py")),
        # A-12: the publication workflow is itself an authority boundary. Adversarial mutations
        # hold its tag-selected dispatch, commit binding, finite frontier, GitHub release order
        # and prohibition on broader publicity;
        # the structural check then holds the checked-in workflow to that contract.
        Step(
            "release workflow tests",
            "gate",
            ("python3", "scripts/test-release-workflow.py"),
        ),
        Step(
            "release workflow",
            "gate",
            ("./scripts/check-release-workflow.py", "--check"),
        ),
        # A-10/P-14: native builders are hosted per target, but deterministic archives, exact
        # SPDX closure, static-linkage refusal, bounded call smoke and retry bytes are portable.
        # Their adversarial fixtures run here so the tag workflow is not their first exercise.
        Step(
            "release artifact tests",
            "gate",
            ("python3", "scripts/test-release-artifacts.py"),
        ),
        # X-38: the surface checker decides which crates are on the reachable-from-a-call surface,
        # which makes its own bugs invisible — both the ones it had while being written reported
        # *nothing*, and a checker with no output looks exactly like a clean tree.
        Step("app surface tests", "gate", ("python3", "scripts/test-app-surface.py")),
        # The interop harness reserves machine-global things and used to let two runs share them,
        # which `X-23` measured as both call tests timing out together. The suite stubs the
        # container runtime, so it belongs beside the others rather than in the `interop` job.
        Step("interop harness tests", "gate", ("python3", "scripts/test-interop-run.py")),
        # P-13: this checker decides whether executable help and versioned output producers still
        # agree with the public reference. Its reversed fixtures belong here because a checker that
        # silently overlooks a command or schema is indistinguishable from an agreeing one.
        Step("cli reference tests", "gate", ("python3", "scripts/test-cli-reference.py")),
        # P-13: the proof runner maps the normative DPH vectors and wider release matrix onto
        # executable process tests and independent-peer profiles. Test its discovery and failure
        # rules separately so a runner that quietly drops a path cannot bless its own omission.
        Step(
            "diagnostic phone proof tests",
            "gate",
            ("python3", "scripts/test-diagnostic-phone-proof.py"),
        ),
        # M-51: this reverses the proof harness's trust and lifecycle boundaries. Compatibility
        # is enforced separately by CI's native-browser positives in both roles and its three
        # real product-boundary mutations.
        Step(
            "browser audio proof harness tests",
            "gate",
            ("python3", "scripts/test-browser-audio-proof.py"),
        ),
        # X-71: the provenance gate now has an exception, and an exception is the part that rots.
        # A fourth pathspec added in a hurry, or a scope that stops reaching `git grep`, turns this
        # into a check that reports clean because it looked nowhere. The suite builds throwaway
        # repositories rather than trusting the real one, so it needs no denylist to run.
        Step("provenance tests", "gate", ("python3", "scripts/test-provenance.py")),
        # X-72: the comparison registry is the one measurement in this repository whose subject is
        # mostly software we do not control, so its checker carries more of the weight than usual —
        # the confidence ladder, the staleness limit and the rule that this repository's own column
        # is substituted rather than typed are all it. A guard that elaborate needs its own suite.
        Step("comparison tests", "gate", ("python3", "scripts/test-comparison-report.py")),
        Step(
            "provenance",
            "provenance",
            ("./scripts/check-provenance.sh",),
            differs=(
                (
                    "--history",
                    "CI checks out the full history and scans it; locally the pre-commit hook "
                    "covers each commit as it is written",
                ),
            ),
        ),
        # X-44: `docs/designs/media.md` states the rule normatively — a fixed wall-clock duration
        # may bound a failure or define silence, and may not stand in for a happens-before — and
        # nothing ran it. Two sweeps declared the workspace clean and two violations landed in the
        # wave after the second one. Cheap, needs no toolchain, and reads `src/` as well as tests.
        Step("fixed sleeps", "fixed-sleep", ("./scripts/check-fixed-sleep.py", "--check")),
        # X-56: both RFC corpora are recovered from the RFC's own Appendix A archive rather than
        # transcribed, and the importer's `--check` re-recovers it and diffs it against the tree —
        # the only thing that can tell a fixture edited by hand from the RFC's own bytes, since the
        # suites read whatever is in the directory and pass. The 4475 check ran only inside `fuzz`,
        # which is in `NOT_RUN_LOCALLY`, so no local run covered it; the 5118 one ran nowhere.
        # A step each, so a red result names which corpus drifted.
        #
        # X-58: both steps have to reach `rfc-editor.org` to say anything at all, and a step that
        # could not reach it knows nothing about the corpus. So they disclaim rather than fail —
        # X-34's doctrine in a third place, and the reason `not_a_result` exists. The importers
        # own their own exit codes, which is what makes the claim theirs to make rather than
        # something this script infers from their output.
        Step(
            "rfc 4475 corpus",
            "corpus",
            ("./scripts/import-rfc4475-corpus.sh", "--check"),
            not_a_result="it could not reach the RFC editor, so it read nothing to compare the "
            "committed corpus against",
        ),
        Step(
            "rfc 5118 corpus",
            "corpus",
            ("./scripts/import-rfc5118-corpus.sh", "--check"),
            not_a_result="it could not reach the RFC editor, so it read nothing to compare the "
            "committed corpus against",
        ),
        Step("rfc compliance", "docs", ("./scripts/rfc-report.py", "--check")),
        # X-24: the connection pool key was described in three specs and had been wrong in one of
        # them through two changes to the type. The list is generated from `ConnectionKey` now,
        # and this is what makes a field added to the key fail before it reaches a reader.
        Step("pool key", "docs", ("./scripts/check-pool-key.py", "--check")),
        # X-26: `sipx-audio`'s package description promised G.722 and resampling from the
        # scaffolding commit onward and the crate implements neither. The description is the
        # first string a user meets, and nothing connected it to the code.
        Step("audio claims", "docs", ("./scripts/check-audio-claims.py", "--check")),
        # X-32: "how far is this from 1.0" is answered from the registry and the board rather than
        # estimated. A stale answer is worse than none, because the only decision it feeds is when
        # to cut a release.
        Step("maturity", "docs", ("./scripts/maturity.py", "--check")),
        # X-38, alpha predicate 1: the reachable surface is *defined* as what the shipped
        # application uses, so a crate claiming supported surface nothing reaches — or an
        # application reaching for something still marked experimental — is a red gate.
        Step("app surface", "docs", ("./scripts/check-app-surface.py", "--check")),
        # X-72: X-35 is the scar — hand-maintained public capability tables that sold a
        # DTLS-SRTP path no call could reach. A comparison page is that failure with a larger blast
        # radius, so it is generated and checked like every other published table, and an
        # observation that has aged past its limit fails the build rather than shipping with a note.
        Step("comparison", "docs", ("./scripts/comparison-report.py", "--check")),
        Step("fmt", "fmt", ("cargo", "fmt", "--all", "--check")),
        Step(
            "clippy",
            "clippy",
            ("cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"),
        ),
        Step("test", "test", ("cargo", "test", "--workspace", "--all-features")),
        Step("examples", "test", ("cargo", "build", "--workspace", "--all-features", "--examples")),
        # P-13: execute the built binary's root and subcommand help, then hold the versioned JSON
        # producers against the public contract table. After the workspace builds so this observes
        # the candidate command rather than a parsed copy of its help constants.
        Step("cli reference", "test", ("./scripts/check-cli-reference.py", "--check")),
        # P-13: the structural proof names every diagnostic-phone process vector, every complete
        # product path and two independent profiles for each released signalling transport. The
        # cargo suite above executes its Rust evidence; this check prevents that evidence from
        # disappearing from the release matrix unnoticed.
        Step(
            "diagnostic phone proof",
            "test",
            ("./scripts/diagnostic-phone-proof.py", "--check"),
        ),
        # C-5: the app contract's end-to-end proof — the interpreter driving a real call, with no
        # host, asserted from a shell. It is a `.sh` and not a `#[test]` because what it checks is
        # the *trace the example prints*, which is the thing a reader of the docs actually sees.
        # That also means nothing in cargo's world runs it, so it goes here or it rots. Ordered
        # after `examples`, which builds it with the same feature set, so this is a run and not a
        # compile.
        Step(
            "app contract end to end",
            "test",
            ("bash", "crates/sipx-app-protocol/tests/canned_program.sh"),
        ),
        Step(
            "msrv",
            "msrv",
            ("cargo", f"+{msrv}", "check", "--workspace", "--all-targets", "--all-features"),
            toolchain=msrv,
        ),
        Step("feature matrix", "features", ("./scripts/check-features.sh",)),
        Step("docs site", "site", ("./scripts/build-docs.sh",)),
    ]


# --------------------------------------------------------------------------------------------
# The MSRV toolchain, derived rather than written down
# --------------------------------------------------------------------------------------------


def normalise_version(text: str) -> tuple[int, int, int]:
    """A version with its patch component and one without are the same claim, spelled two ways."""
    parts = [int(part) for part in text.split(".")]
    if not 1 <= len(parts) <= 3:
        raise ValueError(f"{text!r} is not a Rust version")
    return tuple(parts + [0] * (3 - len(parts)))  # type: ignore[return-value]


def workspace_rust_version() -> str:
    return tomllib.loads(MANIFEST.read_text())["workspace"]["package"]["rust-version"]


def msrv_toolchain() -> str:
    """The rustup toolchain name for the workspace's declared floor.

    Spelled with all three components because that is how `ci.yml` pins it, and the two are
    checked against each other.
    """
    major, minor, patch = normalise_version(workspace_rust_version())
    return f"{major}.{minor}.{patch}"


def installed_toolchains() -> list[str] | None:
    """What rustup has, or `None` if there is no rustup to ask."""
    if shutil.which("rustup") is None:
        return None
    result = subprocess.run(
        ["rustup", "toolchain", "list"], capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        return None
    return [line.strip() for line in result.stdout.splitlines() if line.strip()]


def missing_toolchain_problem(installed: list[str] | None, wanted: str) -> str | None:
    """Why the MSRV step cannot run, or `None` if it can.

    Never "skipped". A gate that quietly drops its MSRV check when the toolchain is absent is the
    defect this step exists for, wearing the costume of a convenience.
    """
    if installed is None:
        return (
            f"rustup is not available, so the MSRV check cannot run — and it is not skipped.\n"
            f"      Install rustup from https://rustup.rs, then: rustup toolchain install {wanted}"
        )
    target = normalise_version(wanted)
    for entry in installed:
        # `<version>-x86_64-unknown-linux-gnu`, or `stable-…`, or a name with `(default)` after it.
        candidate = entry.split()[0].split("-")[0]
        try:
            if normalise_version(candidate) == target:
                return None
        except ValueError:
            continue
    return (
        f"the MSRV toolchain {wanted} is not installed, so the MSRV check cannot run — and it is\n"
        f"      not skipped. Install it with: rustup toolchain install {wanted}"
    )


# --------------------------------------------------------------------------------------------
# Reading ci.yml
# --------------------------------------------------------------------------------------------


@dataclass
class Job:
    """One CI job, reduced to the two things the drift check cares about."""

    name: str
    runs: list[str] = field(default_factory=list)
    uses: list[str] = field(default_factory=list)


_JOB = re.compile(r"^  ([A-Za-z0-9_-]+):\s*$")
_RUN = re.compile(r"^(\s*)(?:- )?run:\s?(.*)$")
_USES = re.compile(r"^\s*(?:- )?uses:\s*(\S+)\s*$")
_BLOCK = {"|", "|-", "|+", ">", ">-", ">+"}

#: Below this, the parser has stopped understanding `ci.yml` rather than found a small workflow.
#: A reader that silently finds nothing reports no drift, forever.
_PLAUSIBLE_JOBS = 8


def _block_scalar(lines: list[str], start: int, key_indent: int) -> tuple[str, int]:
    """Collect a `run: |` body: everything indented past the key, blank lines included."""
    body: list[str] = []
    index = start
    while index < len(lines):
        line = lines[index]
        if line.strip() and (len(line) - len(line.lstrip())) <= key_indent:
            break
        body.append(line.strip())
        index += 1
    return "\n".join(part for part in body if part), index


def parse_workflow(text: str) -> dict[str, Job]:
    """The jobs in `ci.yml`, each with the commands it runs and the actions it uses.

    A narrow reader rather than a YAML library, so the check has no dependency of its own — and
    it asserts it found a plausible number of jobs, because the way a reader like this fails is
    by quietly matching nothing.
    """
    lines = text.splitlines()
    jobs: dict[str, Job] = {}
    current: Job | None = None
    in_jobs = False
    index = 0
    while index < len(lines):
        line = lines[index]
        if line.rstrip() == "jobs:":
            in_jobs = True
            index += 1
            continue
        if in_jobs:
            job = _JOB.match(line)
            if job:
                current = Job(job.group(1))
                jobs[current.name] = current
                index += 1
                continue
        if current is not None:
            run = _RUN.match(line)
            if run:
                value = run.group(2).strip()
                if value in _BLOCK:
                    key_indent = len(line) - len(line.lstrip())
                    body, index = _block_scalar(lines, index + 1, key_indent)
                    current.runs.append(body)
                    continue
                current.runs.append(value)
                index += 1
                continue
            uses = _USES.match(line)
            if uses:
                current.uses.append(uses.group(1))
        index += 1

    if len(jobs) < _PLAUSIBLE_JOBS:
        raise ValueError(
            f"read only {len(jobs)} jobs from {WORKFLOW.name}; the reader has drifted from the "
            f"file's shape and would report no drift whatever CI does"
        )
    return jobs


def parse_workflow_env(text: str) -> dict[str, str]:
    """The workflow-level `env:` block, so the gate builds with the flags CI builds with.

    `RUSTFLAGS: -D warnings` is the one that matters: without it an unused import is a warning
    locally and an error on push, which is a green gate and a red CI for the third time.
    """
    lines = text.splitlines()
    values: dict[str, str] = {}
    for index, line in enumerate(lines):
        if line.rstrip() != "env:":
            continue
        for follow in lines[index + 1 :]:
            if not follow.strip():
                continue
            if not follow.startswith("  ") or follow.startswith("   "):
                break
            key, _, value = follow.strip().partition(":")
            values[key.strip()] = value.strip()
        break
    return values


# --------------------------------------------------------------------------------------------
# The drift check
# --------------------------------------------------------------------------------------------


def _tokens(command: str) -> list[str]:
    return command.split()


def _shape(tokens: list[str]) -> tuple[str, ...]:
    """What makes two invocations the same check: the program and its subcommand.

    A toolchain selector is not part of it — `cargo +<msrv> check` and the `msrv` job's
    `cargo check` are the same command run on two toolchains, which is the point.
    """
    words = [token for token in tokens if not token.startswith("+")]
    if not words:
        return ()
    if words[0] in ("cargo", "python3", "python") and len(words) > 1:
        return (words[0], words[1])
    return (words[0],)


def _flags(tokens: list[str]) -> tuple[set[str], list[str]]:
    """Flags before a bare `--`, and everything after it verbatim."""
    words = [token for token in tokens if not token.startswith("+")]
    if "--" in words:
        cut = words.index("--")
        head, tail = words[:cut], words[cut + 1 :]
    else:
        head, tail = words, []
    return {word for word in head if word.startswith("-")}, tail


def _argument_problems(step: Step, job: str, ci_command: str) -> list[str]:
    """Same command, different strength — the quiet half of drift."""
    ci_flags, ci_tail = _flags(_tokens(ci_command))
    step_flags, step_tail = _flags(list(step.command))
    declared = {flag for flag, _ in step.differs}
    problems = []
    for flag in sorted(ci_flags - step_flags - declared):
        problems.append(
            f"CI job `{job}` passes `{flag}` and gate step `{step.name}` does not; add it, or "
            f"declare the difference on the step with a reason"
        )
    for flag in sorted(step_flags - ci_flags - declared):
        problems.append(
            f"gate step `{step.name}` passes `{flag}` and CI job `{job}` does not; the gate would "
            f"be checking something CI never checks"
        )
    if ci_tail != step_tail:
        problems.append(
            f"gate step `{step.name}` ends `-- {' '.join(step_tail)}` and CI job `{job}` ends "
            f"`-- {' '.join(ci_tail)}`"
        )
    return problems


def drift_problems(jobs: dict[str, Job], steps: list[Step]) -> list[str]:
    """Everywhere the gate and `ci.yml` disagree about what gets checked."""
    problems: list[str] = []
    by_job: dict[str, list[Step]] = defaultdict(list)
    for step in steps:
        by_job[step.ci_job].append(step)

    for step in steps:
        if step.ci_job not in jobs:
            problems.append(
                f"gate step `{step.name}` mirrors CI job `{step.ci_job}`, which {WORKFLOW.name} "
                f"does not define"
            )

    for name, job in sorted(jobs.items()):
        if name in NOT_RUN_LOCALLY:
            if name in by_job:
                problems.append(
                    f"CI job `{name}` is both a gate step and listed as not run locally"
                )
            continue
        if name not in by_job:
            problems.append(
                f"CI job `{name}` is not in the gate and is not listed as run only in CI — add a "
                f"step for it, or an entry in NOT_RUN_LOCALLY saying why not"
            )
            continue
        for command in job.runs:
            flat = " ".join(command.split())
            if flat.startswith(IGNORED_RUN_PREFIXES):
                continue
            matched = [
                step for step in by_job[name] if _shape(step.command) == _shape(_tokens(flat))
            ]
            if not matched:
                problems.append(f"CI job `{name}` runs `{flat}` and no gate step does")
                continue
            problems += _argument_problems(matched[0], name, flat)

    for step in steps:
        job = jobs.get(step.ci_job)
        if job is None:
            continue
        if not any(_shape(_tokens(run)) == _shape(step.command) for run in job.runs):
            problems.append(
                f"gate step `{step.name}` runs `{' '.join(step.command)}`, which CI job "
                f"`{step.ci_job}` does not run"
            )

    for name in sorted(NOT_RUN_LOCALLY):
        if name not in jobs:
            problems.append(
                f"NOT_RUN_LOCALLY names `{name}`, which {WORKFLOW.name} no longer defines"
            )

    return problems


def toolchain_problems(jobs: dict[str, Job], rust_version: str) -> list[str]:
    """The `msrv` job's pin and the workspace `rust-version` are one claim in two files."""
    job = jobs.get("msrv")
    if job is None:
        return [f"{WORKFLOW.name} defines no `msrv` job, so the declared floor is never built"]
    pins = [use.split("@", 1)[1] for use in job.uses if use.startswith("dtolnay/rust-toolchain@")]
    if not pins:
        return [f"CI job `msrv` pins no toolchain, so it does not build the declared floor"]
    problems = []
    for pin in pins:
        try:
            matches = normalise_version(pin) == normalise_version(rust_version)
        except ValueError:
            problems.append(
                f"CI job `msrv` uses toolchain `{pin}`, which is not a version — it has to pin "
                f"the workspace rust-version {rust_version}"
            )
            continue
        if not matches:
            problems.append(
                f"CI job `msrv` pins Rust {pin} and the workspace rust-version is {rust_version}; "
                f"CI is checking a floor the crates do not advertise"
            )
    return problems


def gate_section(text: str) -> str | None:
    """`AGENTS.md`'s gate section, from its heading to the next one."""
    _, marker, rest = text.partition("\n## The gate\n")
    if not marker:
        return None
    section, _, _ = rest.partition("\n## ")
    return section


def documentation_problems(text: str) -> list[str]:
    """`AGENTS.md` must point at the gate, not paraphrase it."""
    section = gate_section(text)
    if section is None:
        return ["AGENTS.md has no `## The gate` section"]
    problems = []
    blocks = re.findall(r"```sh\n(.*?)```", section, re.S)
    if not blocks:
        problems.append(f"AGENTS.md's gate section does not show how to run the gate ({ENTRY_POINT})")
    for block in blocks:
        for line in block.splitlines():
            command = line.split("#", 1)[0].strip()
            if not command:
                continue
            if not command.startswith(ENTRY_POINT):
                problems.append(
                    f"AGENTS.md's gate block runs `{command}` directly; it may invoke "
                    f"{ENTRY_POINT} and nothing else, or it becomes a second copy of the step "
                    f"list and falls behind it"
                )
    if "msrv" not in section.lower():
        problems.append("AGENTS.md's gate section does not name the MSRV check")
    return problems


def version_literal_problems(rust_version: str) -> list[str]:
    """The MSRV is derived from `Cargo.toml`, so nothing here or in AGENTS.md may spell it out."""
    pattern = re.compile(rf"(?<![\d.]){re.escape(rust_version)}(?![\d])")
    problems = []
    section = gate_section(AGENTS.read_text())
    if section is not None and pattern.search(section):
        problems.append(
            f"AGENTS.md's gate section writes the MSRV ({rust_version}) down a second time; it is "
            f"read from the workspace rust-version"
        )
    if pattern.search(SELF.read_text()):
        problems.append(
            f"{SELF.name} writes the MSRV ({rust_version}) down a second time; it is read from "
            f"the workspace rust-version"
        )
    return problems


def check() -> int:
    """Everything `--check` verifies. No cargo, so it costs nothing to make it a gate step."""
    rust_version = workspace_rust_version()
    jobs = parse_workflow(WORKFLOW.read_text())
    steps = gate_steps(msrv_toolchain())
    problems = (
        drift_problems(jobs, steps)
        + toolchain_problems(jobs, rust_version)
        + documentation_problems(AGENTS.read_text())
        + version_literal_problems(rust_version)
    )
    if problems:
        print("The gate and CI have drifted apart:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    print(
        f"gate: {len(steps)} steps over {len(jobs)} CI jobs, "
        f"{len(NOT_RUN_LOCALLY)} of them run only in CI, none unaccounted for"
    )
    return 0


# --------------------------------------------------------------------------------------------
# The disk guard
# --------------------------------------------------------------------------------------------
#
# X-34. On one evening — 2026-07-29 — a full disk produced five red gates, and every one of them
# read as a code defect:
#
#   error: failed to create file '…/target/debug/examples/canned_program.d':
#          No such file or directory (os error 2)
#   error: failed to write '…/target/debug/.fingerprint/rand-…/invoked.timestamp'
#   error: extern location for autocfg does not exist: …/libautocfg-….rlib
#
# Not one of those sentences contains the word disk. Two integration gates were re-run and came
# back green unchanged; one implementor run lost its `target/` and then its whole worktree; and
# `X-28` — a correct merge — was one command away from being reverted for a failure in a crate its
# diff never opened. The wasted minutes are not the cost. The cost is that a gate which fails at
# random trains everyone to re-run it instead of believing it, which is the one thing this
# project's discipline rests on not happening.
#
# So: measure what a run costs, refuse to start below it, and when a step dies of the disk anyway,
# say that the run is *not a result* rather than colouring it red.
#
# **A shared `CARGO_TARGET_DIR` was considered and rejected** (the decision X-34 asked for, either
# way). Pointing every worktree at one build directory would hold one copy of the dependency
# artifacts instead of one per worktree, which is most of the ~10 GiB. It was rejected on three
# grounds:
#
# 1. Cargo takes an exclusive lock on the build directory, so concurrent gates serialise —
#    "Blocking waiting for file lock on build directory". This project routinely runs three or
#    more implementor worktrees plus an integration gate; a full gate is minutes of cargo, so
#    sharing converts a parallel fan-out into a queue and every implementor waits on the others.
#    The fan-out is the thing that makes the backlog move.
# 2. It promotes the worst failure of that evening from an accident to a design feature. One
#    worktree's `cargo clean` — or its deletion, which is how implementor worktrees end — would
#    take every other run's artifacts with it. That is precisely occurrence 4: a `.fingerprint`
#    directory vanishing underneath a running cargo.
# 3. The saving is smaller than it looks. Worktrees hold different code, so the workspace crates
#    are re-fingerprinted and rebuilt on each switch; only the dependency artifacts are genuinely
#    shared, and the part of the build that is already shared for free — the registry and git
#    checkouts under `CARGO_HOME` — needs no lock and is not the part that fills the disk.
#
# What is adopted instead is this guard: refuse to start, name the disk, and say where the space
# went. `cargo clean` in a worktree nobody is using buys a run's worth of space and costs nobody
# else anything.

_GIB = 1024**3

#: What one gate run has been measured to cost in build artifacts, in GiB, and where the number
#: came from. The threshold is derived from these — the story's requirement is a measurement
#: "rather than guessed", and this is the measurement.
#:
#: Measured per step in a cold worktree on 2026-07-29, every step green: clippy 0.7, then
#: `cargo test` +8.4 (the expensive one — it links every test binary), examples +0.0, msrv +0.6 (a
#: second toolchain keeps its own artifacts), feature matrix +0.3, docs site +0.5. 10.6 in total.
#:
#: The second entry is the same figure taken from the other end: the integration worktree's
#: `target/` grew from 13 GiB to 22 GiB over one evening's runs, because nothing there had
#: previously linked the integration test binaries. Both say a run costs about ten gigabytes.
MEASURED_GATE_TARGET_GIB = {
    "a full gate run in a cold worktree, every step green (2026-07-29)": 10.6,
    "one evening's runs in a warm integration worktree, 13 GiB to 22 GiB (2026-07-29)": 9.0,
}

#: Cargo's peak sits above the size it settles at: a relink writes the new artifact before dropping
#: the old, incremental caches are rewritten in place, and rustc's temporaries live under `target/`
#: too. A fraction rather than a round number of gigabytes, so re-measuring moves the margin with
#: the measurement.
LINK_PEAK_MARGIN = 0.10

#: What one run costs, and what the gate refuses to start below.
GATE_RUN_BYTES = int(max(MEASURED_GATE_TARGET_GIB.values()) * _GIB)
REQUIRED_FREE_BYTES = int(GATE_RUN_BYTES * (1 + LINK_PEAK_MARGIN))

#: Checked between steps rather than the full requirement again — the steps already run do not have
#: to be paid for twice. This is the point below which no step can be believed at all, so a disk
#: that another worktree fills mid-run stops the gate at the next boundary, with a sentence,
#: instead of turning the next step red.
FLOOR_FREE_BYTES = int(GATE_RUN_BYTES * LINK_PEAK_MARGIN)

#: `0` green, `1` the tree is wrong, `2` the run is not a result. The third one is the story: a
#: full disk and a broken diff used to leave the same exit code and print the same way, so nothing
#: — not a human reading the summary, not a script — could tell a finding from a non-finding.
EXIT_GREEN = 0
EXIT_RED = 1
EXIT_INFRASTRUCTURE = 2

#: What a step exits to tell the gate its own run was not a result — the same distinction one
#: level down, spoken by the step instead of inferred about it (`X-58`).
#:
#: `EX_TEMPFAIL` from `sysexits(3)`: "a temporary failure, indicating something that is not really
#: an error... the user is invited to retry". Deliberately not `2`: `tar`, `diff` and `grep` all
#: exit `2` for real trouble, and under `set -e` any of them would hand the gate a disclaimer the
#: script never meant to make. Nothing in this gate's toolchain exits `75` by accident.
#:
#: Only steps that declare `not_a_result` are read this way — see `Step.not_a_result`.
STEP_NOT_A_RESULT = 75

#: A path inside a cargo build directory. The marker is what makes the ENOENT shapes below safe to
#: read as infrastructure: cargo saying it cannot find a file *it wrote itself* is a vanished
#: `target/`, whereas the same message about a path under `crates/` is a real missing source.
_BUILD_PATH = r"(?:[/\\]target[/\\]|\.fingerprint)"

#: Escape sequences, stripped before matching: the gate runs steps under `ci.yml`'s `env:`, which
#: sets `CARGO_TERM_COLOR: always`, so cargo's messages arrive with colour in them.
_ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")

#: How a step's output betrays that the machine and not the tree ended it, each with why that is
#: not the reader's diff. Ordered: the first match is the one reported, so the unambiguous shape
#: comes first.
INFRASTRUCTURE_SHAPES: tuple[tuple[re.Pattern[str], str], ...] = (
    (
        re.compile(r"(?i)no space left on device|\(os error 28\)"),
        "the device is full, so nothing this step did after that point means anything",
    ),
    (
        re.compile(
            rf"(?i)failed to (?:create|write|open|remove|link|copy|rename)\b.*{_BUILD_PATH}"
        ),
        "the path cargo could not write is inside the build directory, so `target/` went away "
        "underneath it — the code is not what failed",
    ),
    (
        re.compile(rf"(?i){_BUILD_PATH}.*no such file or directory"),
        "cargo reports a file it wrote itself as missing, which is a vanished build directory and "
        "not a missing source",
    ),
    (
        re.compile(r"(?i)extern location for \S+ does not exist"),
        "an artifact from earlier in this same run is gone, which is a disappearing build "
        "directory and not a missing dependency",
    ),
)


def human(size: int) -> str:
    """GiB to one decimal — the unit `df` prints and the unit a full disk is argued in."""
    return f"{size / _GIB:.1f} GiB"


def free_bytes(path: pathlib.Path) -> int:
    """Free space on the filesystem holding `path`, or the nearest parent that exists.

    The nearest parent matters: on a cold worktree `target/` is what we are asking about and does
    not exist yet.
    """
    for candidate in (path, *path.parents):
        if candidate.exists():
            return shutil.disk_usage(candidate).free
    return 0


def target_directory(environment: dict[str, str]) -> pathlib.Path:
    """Where this run's artifacts will land.

    `CARGO_TARGET_DIR` is honoured if someone set it — the decision above is that the gate does
    not set it, not that it argues with a caller who has.
    """
    override = environment.get("CARGO_TARGET_DIR")
    return pathlib.Path(override) if override else ROOT / "target"


def disk_problem(free: int, required: int) -> str | None:
    """Why the gate must not start, or `None` if it can — the shape of the MSRV check's answer.

    Never a warning, for the same reason the missing MSRV toolchain is never a skip: a gate that
    starts anyway on a disk that cannot hold the build reports cargo's missing-artifact messages,
    and those read as code defects.

    The threshold and the actual free space are both in the sentence, so nobody has to guess which
    number was the problem or go and run `df` to find out.
    """
    if free >= required:
        return None
    return (
        f"not enough disk to run the gate — refusing to start rather than report.\n"
        f"      {human(free)} free, and a run needs {human(required)}.\n"
        f"      That threshold is measured, not guessed: a gate run leaves about "
        f"{human(GATE_RUN_BYTES)} of build\n"
        f"      artifacts, plus {int(LINK_PEAK_MARGIN * 100)}% for cargo's peak while it links.\n"
        f"      Below it cargo fails with `No such file or directory` on files it wrote itself, "
        f"which reads as\n"
        f"      a code defect and is not one — five gates were misread that way in one evening.\n"
        f"      Every worktree pays for its own `target/`, so `cargo clean` in one nobody is using "
        f"is\n"
        f"      usually the cheapest {human(GATE_RUN_BYTES)} available."
    )


def infrastructure_evidence(output: str) -> tuple[str, str] | None:
    """The first line of a step's output that proves the machine failed, and why it is not a diff.

    `None` when nothing in the output says the machine, and erring that way is deliberate: reading
    a real defect as infrastructure would tell an implementor to re-run instead of to look, which
    is the disease rather than the cure.
    """
    for line in output.splitlines():
        stripped = _ANSI.sub("", line).strip()
        for pattern, why in INFRASTRUCTURE_SHAPES:
            if pattern.search(stripped):
                return stripped, why
    return None


def disclaimed_report(disclaimed: list[tuple[str, str]]) -> str:
    """What to print for steps that told us their own runs were not results (`X-58`).

    Deliberately not `infrastructure_report`, and deliberately not the end of the run. Once
    `target/` is gone every later step fails for the same reason, so the disk guard stops; a step
    that could not reach `rfc-editor.org` says nothing about `cargo clippy`, so the rest of the
    gate still runs and still means what it says. What has to survive is only this step's silence
    — it must not arrive in the summary as `N of M steps failed`, which is a claim about the tree
    that nobody made.
    """
    lines = ["gate: NOT A RESULT — these steps could not reach what they check, and did not fail"]
    lines.extend(f"  {name}: {why}" for name, why in disclaimed)
    lines.extend(
        (
            "",
            "Nothing above is a finding about your changes, and re-running the gate on a machine "
            "that can reach\nthem is the whole fix. Every other step in this run means exactly "
            "what it says.",
        )
    )
    return "\n".join(lines)


def infrastructure_report(step: str, evidence: str, why: str, free: int, required: int) -> str:
    """What to print instead of a red step, when the machine and not the tree ended the run.

    The two must not read alike, and that is the whole point of the story. A red step says the
    diff is wrong; this says the run proved nothing about the diff at all. Printing the first when
    the truth was the second cost an evening of re-runs and nearly cost a correct merge.
    """
    return "\n".join(
        (
            "gate: NOT A RESULT — the machine stopped this run, not the tree",
            f"  it stopped at: {step}",
            f"  the evidence:  {evidence}",
            f"  why that is not your diff: {why}",
            f"  disk now:      {human(free)} free, against the {human(required)} a run needs",
            "",
            "This run proved nothing about the tree, and no step after this one was attempted.",
            "Fix the machine — free space, or find what removed `target/` — and run the gate "
            "again. Nothing above is a finding about your changes.",
        )
    )


# --------------------------------------------------------------------------------------------
# Running it
# --------------------------------------------------------------------------------------------


def show(steps: list[Step]) -> int:
    width = max(len(step.name) for step in steps)
    print("  before any step:")
    print(
        f"  {'disk':<{width}}  refuse to start below {human(REQUIRED_FREE_BYTES)} free, and stop "
        f"below {human(FLOOR_FREE_BYTES)} between steps\n"
    )
    for step in steps:
        print(f"  {step.name:<{width}}  {' '.join(step.command)}   (CI: {step.ci_job})")
    print("\n  run only in CI:")
    for name, why in NOT_RUN_LOCALLY.items():
        print(f"  {name:<{width}}  {why}")
    return 0


def run_step(step: Step, environment: dict[str, str]) -> tuple[int, tuple[str, str] | None]:
    """Run one step, streaming its output, and say whether the machine was what killed it.

    The output goes through this process rather than straight to the terminal because classifying a
    failure means reading what it said, and cargo's disk failures are only distinguishable from
    code failures by their text. Colour survives the pipe for the steps that matter: they run under
    `ci.yml`'s `env:`, which sets `CARGO_TERM_COLOR: always`. A shell step that tests for a tty
    itself will print plainly.
    """
    process = subprocess.Popen(
        list(step.command),
        cwd=ROOT,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        errors="replace",
    )
    evidence: tuple[str, str] | None = None
    if process.stdout is not None:
        for line in process.stdout:
            sys.stdout.write(line)
            sys.stdout.flush()
            if evidence is None:
                evidence = infrastructure_evidence(line)
    return process.wait(), evidence


def run(steps: list[Step]) -> int:
    """Run every step, then say which failed.

    Every step, not up to the first failure: the point of the gate is to be told everything that
    is wrong in one pass, and a gate that stops early is a gate people run once and then work
    around one command at a time.

    The disk is the exception, and X-34 is why. Once the build directory is gone, every remaining
    step fails for the same reason and none of those failures is about the tree — continuing would
    manufacture the wall of misleading red that made a correct merge look broken. So a disk failure
    ends the run, and says so in different words from a red step.

    A step that disclaims its own run (`Step.not_a_result`, X-58) gets the second half of that and
    not the first: it is kept out of the red tally, because it made no claim about the tree, but
    the run continues, because its reason does not generalise to the steps after it. If nothing
    else is red the gate exits `EXIT_INFRASTRUCTURE` — this run was not a complete result. If
    something else *is* red the gate exits `EXIT_RED`, because the tree demonstrably is wrong and
    saying "not a result" there would tell an implementor to re-run instead of to look, which is
    the disease X-34 named rather than the cure.
    """
    environment = dict(os.environ)
    for key, value in parse_workflow_env(WORKFLOW.read_text()).items():
        environment.setdefault(key, value)
    target = target_directory(environment)

    free = free_bytes(target)
    print(
        f"\n\033[1m=== disk\033[0m  {human(free)} free, {human(REQUIRED_FREE_BYTES)} needed",
        flush=True,
    )
    problem = disk_problem(free, REQUIRED_FREE_BYTES)
    if problem is not None:
        print(f"  {problem}", file=sys.stderr, flush=True)
        return EXIT_INFRASTRUCTURE

    failed: list[tuple[str, str]] = []
    disclaimed: list[tuple[str, str]] = []
    for step in steps:
        free = free_bytes(target)
        if free < FLOOR_FREE_BYTES:
            return stop_without_a_result(
                step.name,
                f"{human(free)} free with `{step.name}` still to run, below the "
                f"{human(FLOOR_FREE_BYTES)} floor a run needs to keep going",
                "the disk drained below the margin a run still needs while the gate was going, so "
                "this step was not started rather than allowed to fail for it",
                free,
                failed,
            )
        print(f"\n\033[1m=== {step.name}\033[0m  {' '.join(step.command)}", flush=True)
        if step.toolchain:
            toolchain = missing_toolchain_problem(installed_toolchains(), step.toolchain)
            if toolchain is not None:
                print(f"  {toolchain}", file=sys.stderr, flush=True)
                failed.append((step.name, "the toolchain it needs is not installed"))
                continue
        code, evidence = run_step(step, environment)
        if code == 0:
            continue
        if evidence is not None:
            return stop_without_a_result(step.name, *evidence, free_bytes(target), failed)
        if step.not_a_result and code == STEP_NOT_A_RESULT:
            disclaimed.append((step.name, step.not_a_result))
            continue
        failed.append((step.name, f"exit {code}"))

    print()
    if disclaimed:
        print(f"\033[33m{disclaimed_report(disclaimed)}\033[0m", file=sys.stderr)
    if failed:
        print(f"\033[31mgate: {len(failed)} of {len(steps)} steps failed\033[0m", file=sys.stderr)
        for name, why in failed:
            print(f"  {name}: {why}", file=sys.stderr)
        return EXIT_RED
    if disclaimed:
        return EXIT_INFRASTRUCTURE
    print(f"\033[32mgate: {len(steps)} steps, all green\033[0m")
    return EXIT_GREEN


def stop_without_a_result(
    step: str, evidence: str, why: str, free: int, failed: list[tuple[str, str]]
) -> int:
    """End the run as a non-result, and keep whatever real findings came before it.

    Steps that were already red might be genuine, so they are not thrown away — but they are
    reported as unfinished business under a heading that does not claim the run means anything,
    rather than as `N of M steps failed`.
    """
    print()
    print(
        f"\033[33m{infrastructure_report(step, evidence, why, free, REQUIRED_FREE_BYTES)}\033[0m",
        file=sys.stderr,
    )
    if failed:
        print("\n  red before the disk gave out, and worth re-reading on a machine that has room:",
              file=sys.stderr)
        for name, reason in failed:
            print(f"  {name}: {reason}", file=sys.stderr)
    return EXIT_INFRASTRUCTURE


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify the gate still matches ci.yml, and run nothing",
    )
    parser.add_argument("--list", action="store_true", help="print the steps and their CI jobs")
    args = parser.parse_args()

    if args.check:
        return check()
    steps = gate_steps(msrv_toolchain())
    if args.list:
        return show(steps)
    return run(steps)


if __name__ == "__main__":
    sys.exit(main())
