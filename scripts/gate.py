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


#: CI jobs that are deliberately not part of the local gate. Every job in `ci.yml` is either a
#: step above or an entry here — the check enforces that, so adding a job forces the decision.
NOT_RUN_LOCALLY = {
    "interop-peers": "reads the peer list for the matrix below; it verifies nothing itself",
    "interop": "needs a running container per peer; `tests/interop/run.sh --peer <name>` on demand",
    "soak": "minutes per run, which is why it is scheduled nightly rather than run per push",
    "fuzz": "a nightly toolchain and a cargo-fuzz install, for a run that is time-boxed anyway",
    "deny": "runs as a packaged action against a freshly fetched advisory database, not local state",
    "deploy-site": "publishes what `site` built; there is nothing to verify",
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
        # The interop harness reserves machine-global things and used to let two runs share them,
        # which `X-23` measured as both call tests timing out together. The suite stubs the
        # container runtime, so it belongs beside the others rather than in the `interop` job.
        Step("interop harness tests", "gate", ("python3", "scripts/test-interop-run.py")),
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
        Step("rfc compliance", "docs", ("./scripts/rfc-report.py", "--check")),
        # X-24: the connection pool key was described in three specs and had been wrong in one of
        # them through two changes to the type. The list is generated from `ConnectionKey` now,
        # and this is what makes a field added to the key fail before it reaches a reader.
        Step("pool key", "docs", ("./scripts/check-pool-key.py", "--check")),
        # X-26: `sipx-audio`'s package description promised G.722 and resampling from the
        # scaffolding commit onward and the crate implements neither. The description is the
        # first string a user meets, and nothing connected it to the code.
        Step("audio claims", "docs", ("./scripts/check-audio-claims.py", "--check")),
        Step("fmt", "fmt", ("cargo", "fmt", "--all", "--check")),
        Step(
            "clippy",
            "clippy",
            ("cargo", "clippy", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings"),
        ),
        Step("test", "test", ("cargo", "test", "--workspace", "--all-features")),
        Step("examples", "test", ("cargo", "build", "--workspace", "--all-features", "--examples")),
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
# Running it
# --------------------------------------------------------------------------------------------


def show(steps: list[Step]) -> int:
    width = max(len(step.name) for step in steps)
    for step in steps:
        print(f"  {step.name:<{width}}  {' '.join(step.command)}   (CI: {step.ci_job})")
    print("\n  run only in CI:")
    for name, why in NOT_RUN_LOCALLY.items():
        print(f"  {name:<{width}}  {why}")
    return 0


def run(steps: list[Step]) -> int:
    """Run every step, then say which failed.

    Every step, not up to the first failure: the point of the gate is to be told everything that
    is wrong in one pass, and a gate that stops early is a gate people run once and then work
    around one command at a time.
    """
    environment = dict(os.environ)
    for key, value in parse_workflow_env(WORKFLOW.read_text()).items():
        environment.setdefault(key, value)

    failed: list[tuple[str, str]] = []
    for step in steps:
        print(f"\n\033[1m=== {step.name}\033[0m  {' '.join(step.command)}", flush=True)
        if step.toolchain:
            problem = missing_toolchain_problem(installed_toolchains(), step.toolchain)
            if problem is not None:
                print(f"  {problem}", file=sys.stderr, flush=True)
                failed.append((step.name, "the toolchain it needs is not installed"))
                continue
        result = subprocess.run(list(step.command), cwd=ROOT, env=environment, check=False)
        if result.returncode != 0:
            failed.append((step.name, f"exit {result.returncode}"))

    print()
    if failed:
        print(f"\033[31mgate: {len(failed)} of {len(steps)} steps failed\033[0m", file=sys.stderr)
        for name, why in failed:
            print(f"  {name}: {why}", file=sys.stderr)
        return 1
    print(f"\033[32mgate: {len(steps)} steps, all green\033[0m")
    return 0


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
