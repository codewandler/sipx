---
id: X-118
title: Make bounded-timeout tests robust under load
pillar: Build
status: ready
priority: 31
design:
epic: test-surfaces
areas: [sipx-cli, ci]
predicate:
announcement:
note: a CANCEL test that passes in isolation timed out under three concurrent gate runs · a flaky red is worse than a slow one
---

# Make bounded-timeout tests robust under load

## Goal

Stop tests that assert a bounded wall-clock from failing because the machine was busy, without
weakening what they assert.

## Acceptance

- [x] `interrupting_a_pending_dial_cancels_without_manufacturing_a_bye` and every sibling asserting
      a wall-clock bound either use a controllable clock or state a tolerance derived from the
      machine, not a fixed number tuned on an idle box.
- [x] A failing-first proof runs the suite under deliberate CPU contention and shows the assertion
      surviving, while a genuinely unbounded operation still fails it.
- [x] `check-fixed-sleep.py` stays green — this must not become a sleep.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `M-41`'s adjacent findings. Its intermediate gate run hit `test: exit 101`
  on that CANCEL test while three gates ran concurrently; the test passes in isolation and the final
  run on a quiet box had all 145 suites green. `M-41`'s diff touches no `sipx-cli` file.

- 2026-08-08: **stronger evidence, and a corrected cause.** The box this was observed on is shared:
  alongside seven sipx implementor worktrees, five other checkouts (`flux`, `flux-c728`, `flux-c736`,
  `flux-c740`, `flux-c742`) were running their own builds and gates, at load average **41.85 on 20
  cores**. So the CANCEL timeout was not "three concurrent sipx gates" — it was an oversubscribed
  machine, which is both more likely to recur and entirely outside this repository's control. A
  wall-clock assertion tuned on an idle box is not a property of the code under test.

- 2026-08-08: **a deterministic cause has been found for the same symptom, and it was never ruled
  out against the load diagnosis.** `X-121` traced it: `check-cli-reference.py` builds `sipx-cli`
  with default features, and `gate.py` runs that step *after* the all-features `test` step, so every
  gate run leaves a binary the next run's process tests spawn — failing as "heard no audio at all",
  which is exactly what this story attributes to contention. `X-124` owns the fix and `X-121`'s
  guard now makes the two distinguishable. **Do this story after those**, and re-measure whether a
  load-sensitive assertion remains once the deterministic cause is gone; it may be that little or
  none of what was attributed to load was load.

- 2026-08-08: **a third instance, and a new shape.** The `rc.8` gate failed on
  `register::tests::every_exit_joins_the_endpoint_before_the_terminal_record` with `bind: Address
  already in use (os error 98)` — `P-27`'s join probe rebinds the endpoint's port to prove it was
  released, and under the full workspace run something else took the port first, so the command
  reported `failed`/1 where the test expected `timeout`/5. It passes in isolation and across the
  whole `sipx-cli` package suite (91 of 91). So this is not a wall-clock assertion at all: it is a
  **port-reuse race**, which belongs in this story's scope but is a different mechanism from the
  timing ones, and needs a different fix — a probe that observes release without competing for the
  port, or one that tolerates losing the race without changing the assertion's meaning.

- 2026-08-08: attempted the port-race half and backed it out. A bounded retry is the right shape —
  a bind conflict is environmental and the join barrier would still be asserted on every attempt
  that actually ran — but the four sections of
  `register::tests::every_exit_joins_the_endpoint_before_the_terminal_record` capture no output, so
  detecting "the command lost the bind" means restructuring each section to capture its report
  before it can be retried. That is the story's real work rather than a helper, and a helper with no
  call sites is dead code that reads as progress. Left unstarted rather than half-mechanised.

- 2026-08-08: **the port-race mechanism is fixed.** The four exit classes of
  `every_exit_joins_the_endpoint_before_the_terminal_record` now run through `join_probe::until_bound`,
  which retries a class up to five times and panics with every code it saw. That is sound rather
  than lenient: the join assertion is unchanged on each attempt that ran, a command exiting the
  wrong way exits the wrong way every time, and a conflict on all five is something holding the
  ephemeral range rather than a flaky port — which the bound keeps visible instead of hanging.
  No sleep was added; `check-fixed-sleep.py` stays green.
  **The wall-clock rows stay open.** The other two mechanisms this story collected — a stale
  uplifted binary (`X-121`, `X-124`) and genuine machine contention — are different failures with
  different fixes, and the tolerance-versus-controllable-clock question for assertions like the
  CANCEL timeout is untouched here.

## Notes

- This matters more now: concurrent implementors are the normal working mode, so a load-sensitive
  test produces reds nobody caused and everybody has to investigate.

- 2026-08-08: **the contention claim is now measured rather than assumed, and it was true.**
  `scripts/contention-proof.py` loads the box with two CPU burners per core, runs the three bounded
  CLI assertions this story collected, and — in the same run — runs a control that waits on
  `std::future::pending` under the same bound and therefore cannot pass. Both halves are read: a run
  whose control does not go red is reported **inconclusive**, not proven, because a harness that
  cannot fail is green for the same reason an empty suite is. That distinction is the script's whole
  product and its suite (11 tests) is about nothing else.
  **Failing-first, on the real thing.** The first run at 40 burners on 20 cores:
  `contention proof: failed — the control failed as it must ... and under that load these bounded
  assertions failed too: interrupting_a_waiting_answerer_reports_after_listener_cleanup`.
  So the flakiness this story was filed about was real, and separate from the two deterministic
  causes (`X-121`/`X-124`'s stale binary, and the port race fixed above).
  **The fix is a tolerance derived from the machine, which is what the first acceptance row asked
  for.** `tests/support/machine.rs` measures what starting one `sipx version` process costs on
  *this* box, medians five samples, and divides by the at-rest cost to get a scale clamped to 1..=12.
  All 96 process-wait bounds in `tests/cli.rs` now read `bound(Duration::from_secs(n))`. Nothing
  asserted after a wait changed; the only outcome the scale converts is a timeout, on a machine
  measured to need it, and the ceiling keeps a genuinely unbounded operation reported as one.
  Second run, same load: `contention proof: proven — 3 bounded assertions held under the load, in
  the same run where the control was reported red`.
  **One bound was deliberately left unscaled.** `dial`'s 12-second assertion at `cli.rs:7225`
  separates our 3 s schedule from the transaction's 32 s, and its existing comment already reasons
  that load can only push it up so a starved run fails there rather than passing wrongly. Scaling it
  to 144 s would destroy that discrimination. Left as written, and now recorded as a decision.
  `check-fixed-sleep.py` stays green at 43 of 43; the gate is 44 steps and `--check` reports parity.
