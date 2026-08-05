---
id: X-106
title: Measure call-audio analysis accuracy and resource cost
pillar: Build
status: backlog
priority: 27
design: docs/designs/call-audio-analysis.md
epic: call-audio-analysis
areas: [testkit, audio-analysis, vad, metrics, m16]
predicate:
announcement:
note: after M-58 through M-61 · versioned corpus, error rates, event latency, CPU and memory
---

# Measure call-audio analysis accuracy and resource cost

## Goal

Publish a reproducible corpus and resource profile that say how useful the deterministic algorithms
are and prevent “voice activity” from becoming an unmeasured claim.

## Acceptance

- [ ] A versioned, redistributable corpus records sample format, reference activity intervals,
      silence/clipping labels, provenance and licence without embedding private call recordings.
- [ ] The measurement reports false-positive and false-negative duration, start/end sample error,
      clipping/silence agreement, CPU time and peak memory under fixed and calibrated profiles.
- [ ] Thresholds and acceptable budgets are declared before the final run; the command exits nonzero
      when accuracy, latency or resource limits regress.
- [ ] Results are deterministic for the same corpus and configuration and include exact tree,
      platform, profile and command metadata.
- [ ] A bounded long-duration synthetic fixture proves memory and event volume do not grow with call
      duration and records backpressure/discontinuity counts.
- [ ] The generated report is linked from the design and example, checked for drift and green in the
      full gate.

## Progress

- Backlog. M16 analysis measurement after M-58 through M-61.
