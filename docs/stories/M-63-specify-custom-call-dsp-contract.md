---
id: M-63
title: Specify the custom call-DSP contract
pillar: Media
status: backlog
priority: 37
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-audio, sipx-media, dsp, m18]
predicate:
announcement:
note: after M-54 · M18 admission · frame contract, execution profiles and minimum failure policy
---

# Specify the custom call-DSP contract

## Goal

Define one deterministic processor interface shared by built-in effects, noise reduction and
application-supplied DSP before any live-call attachment is implemented.

## Acceptance

- [ ] A normative spec defines PCM format/channel metadata, direction, sample position,
      discontinuity, finite parameter state, output shape and typed observations.
- [ ] Capability discovery declares accepted formats, maximum frame, scratch/state bound,
      algorithmic latency/tail, length preservation and reset/flush behavior.
- [ ] The contract names proven-inline, supervised-isolated and trusted cooperative-native execution
      profiles, including deadline action and fail-open/fail-closed policy; only the first two may
      claim that over-budget work cannot stall RTP.
- [ ] The processor performs no I/O, clock read, task spawn or device discovery; time is sample
      position/rate input and the same vectors are deterministic under virtual or wall time.
- [ ] Invalid format, frame, channel, parameter and discontinuity inputs return typed errors without
      partial state mutation, panic, unsafe code or unbounded allocation.
- [ ] A conformance harness accepts an external fixture processor and proves reset, cancellation,
      chunk-boundary and allocation invariants.
- [ ] Public API docs, byte/sample vectors and the full gate are green.

## Progress

- Backlog. M18 spec and minimum failure-policy gate after M-54's PCM seam.
