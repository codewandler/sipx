---
id: X-99
title: Run and publish the comparative load result
pillar: Build
status: done
priority: 12
design: docs/designs/comparative-load.md
epic: comparative-load
areas: [load, comparison, website, m14]
predicate:
announcement:
note: after M13, X-98 and P-15 · immutable builds, both directions, raw evidence before summary
---

# Run and publish the comparative load result

## Goal

Run the neutral profile against exact immutable endpoint builds in both caller directions and publish
a reproducible result whose limitations are as visible as its measurements.

## Acceptance

- [x] Exact revisions, release builds, toolchains, features, host, kernel, CPU policy, socket limits,
      commands, seeds and artifact hashes are recorded in comparison data.
- [x] One hundred low-rate dialogs qualify protocol correctness in each supported direction before
      capacity work. A failed preflight is "not measured: correctness prerequisite failed", never a
      performance number.
- [x] The neutral driver proves at least twice the tested ceiling under its headroom threshold, then
      runs the fixed finite ladder and all five repetitions without maintaining concurrency by raising
      offered load as a target slows.
- [x] Raw per-run JSON includes all contract fields and cleanup evidence; the generated summary shows
      median and spread, labels uncertainty overlap inconclusive, and never claims an overall winner.
- [x] Both UAC-to-UAS directions run where each build supports them. Missing direction or internal
      state visibility is disclosed and cannot be inferred from the measured direction.
- [x] The first result is explicitly UDP dialog signalling without SDP or media. Secure transports,
      connection churn and audio are not inferred from it.
- [x] The comparison checker validates freshness and hashes, the public site is regenerated, and
      `./scripts/gate.py` is green.

## Progress

- Done. The correctness-qualified, hash-pinned responder run completed the whole fixed ladder,
  generates the internal and public non-ranking summaries, and passed the 36-step repository gate.
