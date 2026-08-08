---
id: M-59
title: Report call signal level clipping and silence metrics
pillar: Media
status: in-progress
priority: 14
design: docs/designs/call-audio-analysis.md
epic: call-audio-analysis
areas: [sipx-audio, sipx-call, app-sdk, audio-analysis, metrics, m16]
predicate:
announcement:
note: after M-57 and M-54 · signal content only, distinct from M-10 network quality
---

# Report call signal level clipping and silence metrics

## Goal

Expose deterministic energy, level, clipping and silence-window observations for call audio while
keeping them distinct from packet-loss, jitter, round-trip and MOS statistics.

## Acceptance

- [ ] Failing-first sample vectors pin the exact energy/level units, window boundaries, clipping
      definition and silence transition for each supported PCM format and rate.
- [ ] Typed observations carry call, direction, sequence, sample time and window coverage through
      `CallEvent` and generated SDK bindings.
- [ ] Discontinuity, format change, reset and cancellation produce documented window behavior and
      cannot report samples from an earlier call or format.
- [ ] Metric events use a bounded cadence/coalescing policy and cannot block RTP or grow with call
      duration.
- [ ] Names and documentation explicitly separate signal-content metrics from M-10's RTP/RTCP
      network-quality snapshot; no existing field changes meaning.
- [ ] Property tests cover extreme samples without overflow or panic and the full gate is green.

## Progress

- Backlog. Depends on M-57 and M-54.
