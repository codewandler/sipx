---
id: M-58
title: Detect voice activity with typed call events
pillar: Media
status: backlog
priority: 23
design: docs/designs/call-audio-analysis.md
epic: call-audio-analysis
areas: [sipx-audio, sipx-call, app-sdk, audio-analysis, vad, m16]
predicate:
announcement:
note: after M-57 and M-54 · start, end and hangover through CallEvent and SDK
---

# Detect voice activity with typed call events

## Goal

Report deterministic voice-start, voice-end and hangover transitions for live call audio without a
speech model or a device-specific runtime.

## Acceptance

- [ ] A failing-first sample corpus produces stable voice-start and voice-end events at the sample
      positions specified by M-57, including the declared hangover behavior.
- [ ] Direction, call identity, observation sequence and sample time reach `CallEvent` and generated
      SDK bindings without polling or an implementation-specific handle.
- [ ] Silence, discontinuity, format change, reset and call cancellation each have one documented
      transition sequence and cannot leave activity latched after teardown.
- [ ] Event delivery is bounded and cannot block call media; coalescing/drop policy preserves the
      latest state and terminal reset.
- [ ] Tests cover two simultaneous calls and prove their calibration, sequence and events never
      cross; no fixed wall-clock sleep establishes ordering.
- [ ] Existing DTMF events remain unchanged, generated bindings and docs are updated, and the full
      gate is green.

## Progress

- Backlog. Depends on M-57 and M-54.
