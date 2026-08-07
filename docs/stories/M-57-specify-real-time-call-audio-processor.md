---
id: M-57
title: Specify deterministic real-time call-audio processing
pillar: Media
status: in-progress
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

- [x] A normative spec defines PCM frame, sample-rate, sequence, direction, discontinuity, reset and
      observation types without sockets, device I/O, clock reads or background tasks.
- [x] Every window, hangover and timeout is expressed in sample counts derived from the declared
      rate, and identical inputs produce identical output events on every machine.
- [x] Memory, CPU work per frame and event queues are explicitly bounded; malformed format changes
      and discontinuities have typed reset/refusal behavior.
- [x] The spec assigns the shared attachment to M-54 and prohibits a second call-media tap or direct
      mutation of provider, playback or RTP state.
- [x] Byte-level/sample-level vectors cover silence, speech-like energy, clipping, impulses, DC,
      format changes and sequence gaps before implementation.
- [ ] The public API review and focused spec/vector checks are green.

## Progress

- The normative contract is [`docs/specs/call-audio-processing.md`](../specs/call-audio-processing.md):
  input contract with direction, rate, sequence and typed discontinuities (§3), the determinism and
  exact duration-derivation rules (§4), integer window predicates with an i64 width proof (§5), the
  activity/hangover/silence-timeout state machine (§6), typed reset versus refusal (§7), memory/CPU/
  queue bounds with a deterministic overflow marker (§8), the `M-54` seam assignment and one-tap
  prohibition (§9), and `CAP-*` sample-level vectors (§11) written before any implementation.
- The design doc names the spec as contract of record. Implementation is `M-58`, `M-59`, `M-60`,
  `M-61` over `M-54`'s seam; the spec's §10 assigns each its slice.
- The last acceptance row stays open deliberately: the spec/vector checks that exist today
  (provenance, docs links, site build) run in the gate, but a public API review needs the API the
  implementing stories will propose — it cannot be green before `M-54`/`M-58` exist.
