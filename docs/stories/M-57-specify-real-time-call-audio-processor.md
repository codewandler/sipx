---
id: M-57
title: Specify deterministic real-time call-audio processing
pillar: Media
status: backlog
priority: 20
design: docs/designs/call-audio-analysis.md
epic: call-audio-analysis
areas: [sipx-audio, sipx-media, audio-analysis, vad, m16]
predicate:
announcement:
note: M16 spec gate · sans-I/O bounded frame processor using M-54's shared seam
---

# Specify deterministic real-time call-audio processing

## Goal

Define a small sans-I/O frame-processing contract for deterministic voice activity and signal facts
before implementing algorithms or SDK events.

## Acceptance

- [ ] A normative spec defines PCM frame, sample-rate, sequence, direction, discontinuity, reset and
      observation types without sockets, device I/O, clock reads or background tasks.
- [ ] Every window, hangover and timeout is expressed in sample counts derived from the declared
      rate, and identical inputs produce identical output events on every machine.
- [ ] Memory, CPU work per frame and event queues are explicitly bounded; malformed format changes
      and discontinuities have typed reset/refusal behavior.
- [ ] The spec assigns the shared attachment to M-54 and prohibits a second call-media tap or direct
      mutation of provider, playback or RTP state.
- [ ] Byte-level/sample-level vectors cover silence, speech-like energy, clipping, impulses, DC,
      format changes and sequence gaps before implementation.
- [ ] The public API review and focused spec/vector checks are green.

## Progress

- Backlog. M16 call-audio-analysis admission gate.
