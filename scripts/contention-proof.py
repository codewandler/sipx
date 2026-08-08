#!/usr/bin/env python3
"""Run the wall-clock-bounded CLI tests on a deliberately oversubscribed machine, with a control.

`X-118` collected three different failures under one symptom — "the gate is flaky". Two had
deterministic causes and were fixed elsewhere: a stale default-feature binary the next run's process
tests spawned (`X-121`, `X-124`), and an ephemeral-port collision in the register join barrier
(fixed in `join_probe::until_bound`). What is left is the original claim, and it is the one nobody
had evidence for: that a bounded-timeout assertion fails because the machine was busy.

That claim can only be settled by making the machine busy on purpose. This script does that.

# Why there is a control, and why it is the point

A proof of the shape "I loaded the box and the tests stayed green" is worth nothing on its own,
because a harness that cannot report a failure is green for the same reason an empty test suite is.
This repository has been burned by exactly that three times — a coverage figure that counted its own
tests, twelve of twenty browser assertions that could not fail, and a discard guard that scanned one
directory level — and each time the measurement was believed because it was green.

So the run has two halves, and **both** must land:

    every subject test must PASS under the load, and
    the control test must FAIL under the same load, in the same run.

The control (`CONTROL` below) is an `#[ignore]`d test that waits on `std::future::pending` under the
same five-second bound the CANCEL test uses. It has no way to pass. If it passes, or is skipped, or
never ran, the verdict is INCONCLUSIVE rather than proven — the harness did not demonstrate it could
go red, so its green says nothing about the subjects. That distinction is the whole product here.

# What the load is

`--factor` processes per core, each spinning on the CPU, for the duration of the cargo run. Two per
core is the default because that is the shape of the box this was observed on: seven sipx worktrees
and five unrelated checkouts building at once, load average 41.85 on 20 cores. The burners are
started before cargo and terminated in a `finally`, so an interrupted run does not leave them behind.

The load is not a fixed sleep and asserts nothing by itself: it is the condition the assertions are
evaluated under, which is what the story asked for.

# Costs, stated

This takes minutes and rebuilds nothing it does not have to, so it is **not** a gate step. `--check`
is: it resolves every subject and control name against the source and verifies the control is still
ignored and still unbounded, in milliseconds, so the proof cannot rot into naming tests that no
longer exist. A renamed test is the way this script would quietly stop proving anything.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: One CPU burner. Spelled out rather than imported so the load does not depend on what is
#: installed; `pass` in a loop is a full core in CPython and needs nothing.
BURNER = "while True: pass"


@dataclass(frozen=True)
class Subject:
    """A test to run, and where the source that defines it lives."""

    selector: str
    target: str
    name: str
    source: str

    def command(self, ignored: bool = False) -> tuple[str, ...]:
        """The cargo invocation that runs exactly this test and nothing else."""
        command = (
            "cargo",
            "test",
            "-p",
            "sipx-cli",
            "--all-features",
            self.selector,
            self.target,
            "--",
            "--exact",
            self.name,
        )
        return command + ("--ignored",) if ignored else command


#: The assertions this story is about: each bounds a wall clock, and each has been seen red on a
#: busy box at least once.
SUBJECTS = (
    Subject(
        "--test",
        "cli",
        "interrupting_a_pending_dial_cancels_without_manufacturing_a_bye",
        "crates/sipx-cli/tests/cli.rs",
    ),
    Subject(
        "--test",
        "cli",
        "interrupting_a_waiting_answerer_reports_after_listener_cleanup",
        "crates/sipx-cli/tests/cli.rs",
    ),
    Subject(
        "--bin",
        "sipx",
        "register::tests::every_exit_joins_the_endpoint_before_the_terminal_record",
        "crates/sipx-cli/src/register.rs",
    ),
)

#: The deliberate failure. See the module docstring: without this going red, the subjects going
#: green is not evidence.
CONTROL = Subject(
    "--test",
    "cli",
    "contention_control_an_unbounded_wait_is_still_reported",
    "crates/sipx-cli/tests/cli.rs",
)

PROVEN = "proven"
FAILED = "failed"
INCONCLUSIVE = "inconclusive"


def verdict(subjects: dict[str, bool], control_failed: bool) -> tuple[str, str]:
    """Read the two halves of the run into one answer, and say why.

    `subjects` maps a test name to whether it passed. `control_failed` is whether the control went
    red, which is the state it is supposed to be in.

    The control is read **first and unconditionally**. A run where every subject passed and the
    control also passed is not a better result than one where a subject failed — it is a result
    about nothing, and calling it proven would be the exact error this script exists to avoid.
    """
    if not control_failed:
        return (
            INCONCLUSIVE,
            f"the control ({CONTROL.name}) did not fail. It waits on a future that never "
            "completes, so a run where it passes did not run it — and the subjects' green says "
            "nothing about contention until the harness has shown it can report a red one.",
        )
    red = sorted(name for name, passed in subjects.items() if not passed)
    if red:
        return (
            FAILED,
            "the control failed as it must, so the harness reports failures — and under that load "
            f"these bounded assertions failed too: {', '.join(red)}.",
        )
    if not subjects:
        return (
            INCONCLUSIVE,
            "no subject test ran. The control proves the harness works and nothing was measured "
            "with it.",
        )
    return (
        PROVEN,
        f"{len(subjects)} bounded assertions held under the load, in the same run where the "
        "control was reported red — so the harness could have failed them and did not.",
    )


def start_burners(count: int) -> list[subprocess.Popen]:
    """Oversubscribe the machine. The caller must stop these in a `finally`."""
    return [
        subprocess.Popen(
            [sys.executable, "-c", BURNER],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        for _ in range(count)
    ]


def stop_burners(burners: list[subprocess.Popen]) -> None:
    for burner in burners:
        burner.terminate()
    for burner in burners:
        try:
            burner.wait(timeout=10)
        except subprocess.TimeoutExpired:
            burner.kill()


def run(subject: Subject, ignored: bool = False) -> bool:
    """True when the test passed. Cargo's own exit status, never a pipeline's."""
    completed = subprocess.run(
        subject.command(ignored=ignored),
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        check=False,
    )
    return completed.returncode == 0


def check_problems() -> list[str]:
    """Every name this script runs still exists, and the control is still a control.

    Cheap enough for a gate step, and it catches the one way this proof rots silently: a test gets
    renamed, cargo matches nothing, `--exact` reports zero tests run, and cargo exits 0.
    """
    problems: list[str] = []
    for subject in SUBJECTS + (CONTROL,):
        source = ROOT / subject.source
        if not source.exists():
            problems.append(f"{subject.source} does not exist, so {subject.name} cannot be found")
            continue
        text = source.read_text(encoding="utf-8")
        leaf = subject.name.rsplit("::", 1)[-1]
        if not re.search(rf"\bfn {re.escape(leaf)}\b", text):
            problems.append(
                f"{subject.name} is not defined in {subject.source}; `--exact` would match no "
                "test and cargo would exit 0, so the run would report a proof of nothing"
            )
    control = ROOT / CONTROL.source
    if control.exists():
        text = control.read_text(encoding="utf-8")
        body = text[text.find(f"fn {CONTROL.name}") :] if f"fn {CONTROL.name}" in text else ""
        head = text[: text.find(f"fn {CONTROL.name}")] if body else ""
        if "#[ignore" not in head[-600:]:
            problems.append(
                f"{CONTROL.name} is not `#[ignore]`d. A control that fails on purpose must stay "
                "out of every ordinary run, or the suite is permanently red"
            )
        if "pending" not in body[:600]:
            problems.append(
                f"{CONTROL.name} no longer waits on a future that never completes, so it is no "
                "longer guaranteed to fail and cannot serve as the control"
            )
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="resolve every name against the source without running cargo",
    )
    parser.add_argument(
        "--factor",
        type=int,
        default=2,
        help="CPU burners per core (default 2, the shape of the box this was observed on)",
    )
    arguments = parser.parse_args()

    if arguments.check:
        problems = check_problems()
        if problems:
            print("The contention proof names tests it cannot run:", file=sys.stderr)
            for problem in problems:
                print(f"  {problem}", file=sys.stderr)
            return 1
        print(
            f"contention proof: {len(SUBJECTS)} bounded assertions and 1 control, all resolved"
        )
        return 0

    cores = os.cpu_count() or 1
    count = cores * arguments.factor
    print(f"loading {cores} cores with {count} burners, then running the proof", flush=True)
    burners = start_burners(count)
    try:
        control_failed = not run(CONTROL, ignored=True)
        subjects = {subject.name: run(subject) for subject in SUBJECTS}
    finally:
        stop_burners(burners)

    answer, why = verdict(subjects, control_failed)
    print(f"contention proof: {answer} — {why}")
    return 0 if answer == PROVEN else 1


if __name__ == "__main__":
    raise SystemExit(main())
