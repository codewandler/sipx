---
id: A-29
title: Publish a runnable live call-audio analysis example
pillar: Application
status: backlog
priority: 28
design: docs/designs/call-audio-analysis.md
epic: call-audio-analysis
areas: [example, app-sdk, audio-analysis, vad, documentation, m16]
predicate:
announcement:
note: M16 analysis exit after X-106 · no model or special hardware required
---

# Publish a runnable live call-audio analysis example

## Goal

Demonstrate voice activity and signal metrics on a live call through the packaged application SDK,
with no speech model, accelerator or private media API.

## Acceptance

- [ ] The example places or answers a real call and prints typed voice-start/end, level, clipping,
      silence, reset and cancellation events with direction and sample time.
- [ ] It uses only supported call and SDK surfaces, requires no model or special hardware, and does
      not retain or print raw audio.
- [ ] A bounded deterministic test drives the packaged example with labelled audio, proves the event
      order and observes cleanup after hangup without fixed sleeps.
- [ ] The guide distinguishes signal observations from M-10 network quality, explains thresholds
      and links the exact X-106 accuracy and resource measurements.
- [ ] The example shows activity-aware synthesis ducking only when A-27 is available; its basic
      analysis path has no dependency on local speech providers.
- [ ] A clean consumer builds the example, public docs and link checks pass, and the full gate is
      green.

## Progress

- Backlog. Final call-audio-analysis example after X-106.
