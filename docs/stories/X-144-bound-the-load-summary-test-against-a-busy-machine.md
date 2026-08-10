---
id: X-144
title: Bound the load-summary test against a busy machine
pillar: Experience
status: in-progress
priority: 7
design:
epic: conformance
areas: [sipx-cli, tests, contention]
predicate:
announcement:
note: found at the rc.19 boundary · passes 3/3 alone, failed once inside a fully loaded gate
---

# Bound the load-summary test against a busy machine

## Goal

Make `load::tests::the_summary_joins_the_endpoint_before_it_is_printed` decide the question it asks
on a loaded machine, or state the load it requires — rather than reporting a scheduling shortfall as
a failed assertion about outcomes.

## Context

Observed at the `1.0.0-rc.19` boundary on 2026-08-10. The test failed inside a full gate run:

```
assertion `left == right` failed: an admitted call that nothing answered is a measured outcome,
not an internal failure
  left: 1
 right: 0
```

Immediately afterwards it passed **three times out of three** in isolation, and the gate run that
failed it followed five concurrent story implementors, each with its own cargo build, on a box that
had been at high load for hours.

So the assertion is almost certainly right about the code and wrong about the machine: a call the
harness admitted was not answered within whatever window it allows, and the test reads that as an
internal failure because on an idle box the two are the same thing. That is the failure mode this
repository already has machinery for — `crates/sipx-cli/tests/support/machine.rs` derives bounds from
the machine, and `scripts/contention-proof.py` exists to prove bounded assertions survive contention
with a control that must go red. This test is in neither.

**A flaky test that is re-run until green is worse than a failing one**, because the next real
regression here will be read as the same weather. The fix is not a longer sleep: it is either a
machine-derived bound, admission headroom so the workload is a claim about the workload rather than
about scheduling luck, or an explicit refusal to measure when the machine cannot support the
measurement — the pattern the DSP cost harness already uses when it declines to report CPU from a
loaded box.

## Acceptance

- [x] The test's actual contention bound is finite and event-derived: five fresh local-port attempts,
      with no fixed wait, before it refuses to measure a join barrier the machine could not set up.
- [x] Failing-first: the current test is shown to fail under manufactured contention, and the
      repaired one to pass under the same contention — not merely to pass when idle.
- [x] A local-port contention shortfall is distinguishable in the failure message from an admitted
      call that genuinely went unanswered. The previous message asserted the second when setup had
      never admitted a call.
- [x] The test is a subject of `scripts/contention-proof.py`, so this cannot regress silently.
- [ ] The gate is green.

## Progress

- Filed 2026-08-10 from a gate run at the rc.19 boundary. Evidence above: one failure under load,
  three passes in isolation immediately after, no change to `crates/sipx-cli/src/load.rs` in this
  candidate (last touched by `f1d922f4`, before rc.18).

- 2026-08-10: deliberate CPU oversubscription refuted the initial scheduling diagnosis. The exact
  test stayed green with both 40 and 80 burners on this 20-core host. That agrees with the code:
  `--timeout 1` defines the peer's silence, and the command classifies that silence as a measured
  timeout with a successful process exit. It is not a wall-clock assertion that scheduling can
  turn into the reported internal-failure exit.

  The matching mechanism is the local-port reuse race already described by `join_probe::free_local`.
  The failing-first test held the just-released port before `run` could bind it and reproduced the
  filed signature exactly: exit 1 where exit 0 was expected. The captured command record supplied
  the fact the assertion hid: `bind: io: Address already in use (os error 98)`. No call had been
  admitted, so the old message about an unanswered admitted call was false.

  The repaired test deliberately creates that collision on its first attempt, then runs through
  `join_probe::until_bound`. A fresh port gets at most five attempts; every valid attempt still
  asserts that the endpoint released its socket before the summary. Exhaustion names local-port
  contention or an internal error separately from the peer's measured silence. There is no sleep
  and no retry of a call that was actually admitted.

  The exact repaired test is green, the contention-proof checker resolves six subjects and its
  deliberate control, and the full proof is green with 40 burners: all six subjects held in the
  same run where the unbounded control went red. The Python harness suite is 15 of 15. The wave
  coordinator owns the complete gate, so its acceptance row remains open and the story remains
  in progress.

  **Owed CHANGELOG sentence** (the release coordinator owns `CHANGELOG.md`):

  > The load-summary join-barrier test now distinguishes local-port contention from an unanswered
  > admitted call and retries setup within a finite bound before judging the command outcome.
