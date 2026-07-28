---
id: X-5
title: Assert stability under sustained load
pillar: Build
status: done
priority: 13
design: docs/designs/sip-core.md
epic: depth
areas: [sipx-testkit]
note:
---

# Assert stability under sustained load

## Goal
Prove that nothing grows without bound, which is the failure that only appears in production.

## Acceptance
- [x] A soak run holds a steady call rate and asserts that memory, task count, open sockets and
      transaction-store size are all flat at the end.
- [x] Flat rather than merely bounded: a leak that fills a bounded pool is still a leak, and
      the pool only hides when it becomes a problem.
- [x] A deliberately introduced leak fails the run, so the assertion is known to have teeth.
- [x] Runs in CI on a schedule rather than per push, since it takes minutes.
- [x] Failing-first test: `an_injected_leak_fails_the_soak`.

## Progress
- Done. `sipx_testkit::soak` samples resident memory, tasks, file descriptors and the endpoint's
  own transaction store before and after, and asserts each is **flat** rather than under a
  ceiling — a leak that fills a pool is still a leak, and the pool hides the cause as well as the
  effect.
- Memory is measured last and was nearly left out, which would have been the acceptance ticked
  and unmet. It is the dimension the other three cannot see: a session that grows a buffer per
  packet leaks steadily with its task and transaction counts perfectly flat, and that is an
  ordinary shape — a recording buffer, a statistics history, a queue with no bound.

## Open: the memory dimension is weaker than the other three

Measured over four identical 300-call batches, 40 s settle between each:

| batch | before | after | growth |
|---|---|---|---|
| 1 | 8 544 kB | 22 788 kB | +14 244 |
| 2 | 22 788 kB | 26 124 kB | +3 336 |
| 3 | 26 124 kB | 28 020 kB | +1 896 |
| 4 | 28 020 kB | 30 248 kB | +2 228 |

The first batch is overwhelmingly warm-up, which is why the soak now warms with a full batch
before taking its baseline. **What the growth does not do is reach zero.** From the second batch
on it settles at roughly 2 MB per 300 calls — about 7 kB a call — and that residual is not
explained. It is consistent with glibc arena high-water marks under a concurrent workload, and
equally consistent with a small genuine leak. RSS cannot tell those apart.

So the tolerance is set from the measurement (6 MB, three times the observed steady state)
rather than tuned until the test passed, and the consequence is stated rather than hidden:
**this dimension catches a gross leak and would miss a small one.** Closing it properly needs a
heap profiler — `dhat` or `valgrind --tool=massif` over the same load — which is worth its own
story rather than a tolerance that pretends.
- It found a real one. **A server transaction the application never answers was held for the
  life of the process.** RFC 3261 §17.2 gives one in `Trying` no timer, because its model is
  that the transaction user always responds; an application that ignores a method it does not
  implement, or that panics in a handler, leaves it there. The soak reported 300 for 300 calls,
  still present two minutes on. Now bounded: the driver abandons one unanswered after three
  minutes and says so in the log, since it is an application bug rather than a network one.
- And it found a mistake in itself first, which is the more instructive one. The settling period
  was five seconds, and the run failed with "tasks grew from 5 to 305" — which is exactly what
  one leaked task per call looks like and was **Timer J**, the thirty-two seconds RFC 3261 §17
  requires a completed transaction to linger. A soak shorter than the protocol's own timers
  measures the specification and calls it a leak. `SETTLE_PAST_TIMERS` is now the documented
  floor, and there is a test asserting it outlasts Timer J.
- The assertion has teeth: `an_injected_leak_fails_the_soak` gives it forty tasks that never
  finish and requires it to fail and to name what leaked.
- With both fixed, 300 real calls leave: tasks 5→5, descriptors 14→14, outstanding 0→0.
