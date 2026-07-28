---
id: X-4
title: Build the load testing harness
pillar: Build
status: done
priority: 12
design: docs/designs/sip-core.md
epic: depth
areas: [sipx-testkit]
note:
---

# Build the load testing harness

## Goal
Find out what sipx does under load before someone else does.

## Acceptance
- [x] A harness places and answers many concurrent calls, with a configurable rate and count.
- [x] Reports calls per second, setup latency percentiles, and failures by cause — an aggregate
      success rate hides which failure is growing.
- [x] Runs against sipx and against a third-party server, so a limit can be attributed to one
      side or the other.
- [x] Failing-first test: `the_harness_reports_a_failure_it_was_given`.

## Progress
- Done. `sipx_testkit::load` — generic over what "a call" means, so the same harness drives
  sipx against itself and sipx against a third party. That is not generality for its own sake:
  a limit found with sipx on both ends cannot be attributed to either half.
- Two reporting rules, each written down because breaking it makes the numbers worse than
  useless. **Failures by cause, never aggregated** — a run slipping from 99% to 97% may be a new
  failure appearing while an old one recedes, and which is growing is the whole question.
  **Percentiles, never a mean** — setup latency is a tight cluster with a tail of retransmission
  timeouts, and a mean sits in the empty space between them describing a call that never
  happened. There is a test asserting exactly that.
- Paced by an arrival rate rather than by concurrency. A harness that keeps N in flight speeds
  up as the system under test slows down, which is the opposite of applying load.
- Throughput counts successes, not attempts: counting attempts reports the highest number at the
  moment the system stops working.
- Measured against real sipx calls: 300 calls at 30/s, all 300 answered, p50 23 ms, p95 28 ms.
