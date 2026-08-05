---
id: M-67
title: Control call DSP graphs through the application SDK
pillar: Media
status: backlog
priority: 40
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-call, app-sdk, dsp, m18]
predicate:
announcement:
note: after M-64 · typed registry and sample-boundary parameters, never SDK callbacks on media work
---

# Control call DSP graphs through the application SDK

## Goal

Let applications compose and change registered DSP processors while preserving the media worker's
bounded, non-callback execution model.

## Acceptance

- [ ] The SDK exposes processor discovery, graph generation, direction/order, closed parameter
      schemas and typed activation/bypass/failure/removal events.
- [ ] Parameter updates are finite, validated off the media path and applied at a declared sample
      boundary with generation/correlation identity and exactly one terminal outcome.
- [ ] SDK/JavaScript callbacks never execute on the media worker; applications select registered
      processor IDs and receive bounded events rather than borrowing audio-thread objects.
- [ ] Unknown processors/parameters, stale generations, cross-call IDs and overlong graphs are
      refused without changing the active graph.
- [ ] Event queues coalesce safe intermediate parameter state but preserve activation, failure,
      bypass and terminal transitions without blocking media.
- [ ] Generated bindings, sequence tests, public reference docs and the full gate are green.

## Progress

- Backlog. Application surface after M-64.
