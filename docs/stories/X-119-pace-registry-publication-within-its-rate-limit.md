---
id: X-119
title: Pace registry publication within its rate limit
pillar: Build
status: ready
priority: 32
design:
epic: conformance
areas: [scripts, release]
predicate:
announcement:
note: split out of X-93 · the 429 pacing row shares nothing with the rest of that story
---

# Pace registry publication within its rate limit

## Goal

Publish the crate set without tripping the registry's rate limit, and without a fixed sleep standing
in for knowing the limit.

## Acceptance

- [ ] `scripts/release.py` paces publication against the registry's stated limit and its response
      headers, rather than a constant delay between crates.
- [ ] A `429` is retried within a bounded budget and reported as itself; exhausting the budget stops
      before any further publication rather than continuing.
- [ ] A failing-first test drives the pacing from injected responses, including a `429` with and
      without a retry hint, with no wall-clock sleep in the test.
- [ ] Publication remains resumable: a paced run that stops mid-frontier restarts without
      republishing or moving anything already published.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: split out of `X-93` per the rc.4 readiness audit, which found this row shares nothing
  with that story's other four and is story-sized on its own.
