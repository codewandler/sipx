---
id: M-65
title: Ship deterministic audio effects and filters
pillar: Media
status: backlog
priority: 39
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-audio, dsp, filters, effects, m18]
predicate:
announcement:
note: after M-63 · gain/filter/distortion/bit-crush/stutter processors use the public contract
---

# Ship deterministic audio effects and filters

## Goal

Provide a practical built-in effect set without giving those processors private access to the media
runtime.

## Acceptance

- [ ] Built-ins include gain, polarity, hard/soft clipping, bit crushing, bounded delay/stutter
      glitching, and stable high-pass, low-pass and peaking filters.
- [ ] Every processor publishes closed parameter ranges, supported rates/channels, latency, tail,
      reset and smoothing behavior through M-63 capability discovery.
- [ ] Golden impulse, step, frequency and sample vectors prove exact ordering, saturation behavior,
      chunk-boundary independence and deterministic parameter transitions.
- [ ] Extreme amplitude, invalid coefficients/rates, zero/maximum delay and arbitrary finite PCM
      never produce NaN, wraparound, panic or allocation beyond the declared bound.
- [ ] Intentional glitch/stutter output is distinguishable in events/metrics from accidental
      discontinuity or deadline miss; docs never present an overload defect as an effect.
- [ ] Each built-in passes the external-processor conformance harness and the full gate is green.

## Progress

- Backlog. Built-in processor wave after M-63.
