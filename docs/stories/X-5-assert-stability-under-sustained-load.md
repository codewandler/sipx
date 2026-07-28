---
id: X-5
title: Assert stability under sustained load
pillar: Build
status: ready
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
- [ ] A soak run holds a steady call rate and asserts that memory, task count, open sockets and
      transaction-store size are all flat at the end.
- [ ] Flat rather than merely bounded: a leak that fills a bounded pool is still a leak, and
      the pool only hides when it becomes a problem.
- [ ] A deliberately introduced leak fails the run, so the assertion is known to have teeth.
- [ ] Runs in CI on a schedule rather than per push, since it takes minutes.
- [ ] Failing-first test: `an_injected_leak_fails_the_soak`.

## Progress
- Not started.
