---
id: M-54
title: Expose bounded call PCM processing and resampling
pillar: Media
status: ready
priority: 1
design: docs/designs/local-speech.md
epic: local-speech
areas: [sipx-media, sipx-audio, app-sdk, speech, audio-analysis, m16]
predicate:
announcement:
note: after A-25 and M-43 · shared seam for local speech and deterministic analysis
---

# Expose bounded call PCM processing and resampling

## Goal

Attach bounded application media processors to a live call through one direction-aware PCM seam,
reusing M-43's unopinionated format conversion rather than creating speech-specific media plumbing.

## Acceptance

- [ ] A failing-first test attaches processors to received and transmitted audio and observes PCM
      frames with direction, sample format, sample time, sequence and discontinuity metadata.
- [ ] A processor requests one supported sample format and rate; conversion reuses M-43 and a typed
      refusal names unsupported conversion rather than distorting or dropping the call.
- [ ] Per-call queues are finite and a slow processor follows a documented frame-loss policy,
      receives a discontinuity and cannot block RTP decode, encode, playback or capture.
- [ ] Attach, detach, call cancellation and processor failure release every buffer and task, with
      observable completion and no fixed sleep standing in for ordering.
- [ ] Two simultaneous consumers — one speech provider and one deterministic analyser — receive the
      declared fan-out semantics without sharing mutable state across calls.
- [ ] Existing playback, recording, DTMF and RTCP behavior remains green under the full gate.

## Progress

- Backlog. Depends on A-25 and M-43; shared by both M16 epics.
- 2026-08-08: **readiness audit — ready.** One instruction for the implementor: the seam
  specification is in scope, and the loss policy derives from `call-audio-processing.md` §8.3
  together with `speech-providers.md` §8 rather than being invented here.
