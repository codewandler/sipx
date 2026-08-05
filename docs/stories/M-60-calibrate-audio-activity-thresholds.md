---
id: M-60
title: Calibrate and adapt audio-activity thresholds deterministically
pillar: Media
status: backlog
priority: 25
design: docs/designs/call-audio-analysis.md
epic: call-audio-analysis
areas: [sipx-audio, audio-analysis, vad, m16]
predicate:
announcement:
note: after M-58 and M-59 · bounded adaptation with observable reset and limits
---

# Calibrate and adapt audio-activity thresholds deterministically

## Goal

Keep voice activity useful across quiet and noisy calls with bounded, reproducible calibration whose
state and limits are inspectable rather than hidden in wall-clock behavior.

## Acceptance

- [ ] A spec amendment defines calibration samples, update rule, floor, ceiling, maximum movement,
      freeze conditions and reset semantics entirely in sample-count terms.
- [ ] Failing-first vectors prove identical threshold evolution and activity events across runs,
      including noise ramps, sudden steps, long silence and alternating near-threshold input.
- [ ] Call and direction state are independent and bounded; discontinuity, format change and
      cancellation reset exactly the fields the spec names.
- [ ] Applications can inspect the active profile and effective thresholds without mutating internal
      state or receiving raw retained audio.
- [ ] Adaptation cannot convert clipping, DC or a single impulse into an unbounded active interval,
      and arithmetic cannot overflow for any supported sample.
- [ ] X-106's corpus records the calibrated and fixed-profile result separately and the full gate is
      green.

## Progress

- Backlog. Depends on M-58 and M-59.
