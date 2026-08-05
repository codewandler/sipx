---
id: M-56
title: Ship a practical local offline speech-synthesis provider
pillar: Media
status: backlog
priority: 24
design: docs/designs/local-speech.md
epic: local-speech
areas: [sipx-media, speech, synthesis, gpu, cpu, m16]
predicate:
announcement:
note: after A-25, A-28 and M-54 · accelerator path plus defined CPU behavior
---

# Ship a practical local offline speech-synthesis provider

## Goal

Generate intelligible speech into a live call through a local accelerator or declared CPU path,
using the same provider and lifecycle contract available to downstream implementations.

## Acceptance

- [ ] The provider implements A-25 and advertises languages, voices, output formats, streaming,
      accelerator support, CPU support, resource estimates and local/offline operation.
- [ ] A failing-first fixture turns bounded text input into ordered PCM chunks and completes through
      the call-media seam with no unbounded text, audio or request queue.
- [ ] Accelerator selection and explicit fallback behave as specified; unavailable voice, language,
      format or device is a typed refusal and never a silent substitution.
- [ ] The CPU profile either meets a predeclared real-time fixture budget or refuses with the
      measured requirement, without changing voice, quality, privacy or latency policy.
- [ ] Warm-up, cancellation, overload, generation failure and shutdown release all tasks, buffers
      and device resources and emit exactly one terminal provider outcome.
- [ ] Offline packaging, licensing, platform limits and measured quality/latency are documented and
      the provider passes X-105 plus the full gate.

## Progress

- Backlog. Depends on A-25, A-28 and M-54.
