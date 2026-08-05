---
id: A-34
title: Publish a runnable custom call-DSP example
pillar: Application
status: backlog
priority: 42
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [example, app-sdk, dsp, documentation, m18]
predicate:
announcement:
note: M18 exit after M-67 and X-109 · live graph, custom fixture, effects/noise reduction, bypass
---

# Publish a runnable custom call-DSP example

## Goal

Show a clean consumer attaching and changing custom call audio processing through the packaged SDK
surface, with audible/recorded evidence and complete lifecycle visibility.

## Acceptance

- [ ] A bounded example call attaches separate transmit/receive graphs containing a custom fixture,
      built-in filter/effect and the bundled noise reducer, then changes order/parameters live.
- [ ] The example exposes exact capability/parameter schemas, graph generations and
      activated/bypassed/failed/removed events without receiving a media-thread callback.
- [ ] Deterministic mode compares recognizable bypassed and processed fixtures; live mode records
      finite before/after output with explicit consent, path and teardown.
- [ ] It demonstrates an intentional distortion/glitch effect and separately proves that deadline
      misses are reported as failures, not presented as the effect.
- [ ] Cancellation and graph replacement end with zero calls, tasks, frames and processor state, and
      no raw audio is retained by default.
- [ ] A clean packaged consumer runs it from public docs and website source/link/full-gate checks pass.

## Progress

- Backlog. Final M18 public example after M-67 and X-109.
