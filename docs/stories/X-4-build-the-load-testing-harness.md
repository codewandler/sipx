---
id: X-4
title: Build the load testing harness
pillar: Build
status: ready
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
- [ ] A harness places and answers many concurrent calls, with a configurable rate and count.
- [ ] Reports calls per second, setup latency percentiles, and failures by cause — an aggregate
      success rate hides which failure is growing.
- [ ] Runs against sipx and against a third-party server, so a limit can be attributed to one
      side or the other.
- [ ] Failing-first test: `the_harness_reports_a_failure_it_was_given`.

## Progress
- Not started.
