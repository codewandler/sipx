---
id: M-55
title: Ship a practical local offline speech-recognition provider
pillar: Media
status: backlog
priority: 23
design: docs/designs/local-speech.md
epic: local-speech
areas: [sipx-media, speech, recognition, gpu, cpu, m16]
predicate:
announcement:
note: after A-25, A-28 and M-54 · accelerator path plus defined CPU behavior
---

# Ship a practical local offline speech-recognition provider

## Goal

Provide useful streaming recognition for live far-end call audio on a local accelerator, with a
declared CPU-only profile and no implicit network dependency.

## Acceptance

- [ ] The provider implements A-25 and advertises its languages, input formats, streaming behavior,
      accelerator support, CPU support, resource estimate and local/offline property.
- [ ] A failing-first bounded fixture produces ordered partial, replacement and final utterances
      from live-call PCM and preserves discontinuity and cancellation semantics.
- [ ] Accelerator selection is capability-driven; absence or exhaustion produces a typed result and
      follows only an explicitly configured fallback policy.
- [ ] The CPU profile either meets a predeclared real-time fixture budget or refuses setup with the
      measured requirement; it never silently lowers language, quality or privacy policy.
- [ ] Model warm-up, readiness, overload, failure and shutdown are bounded and observable, with no
      leaked task, buffer or device allocation after call cancellation.
- [ ] Offline packaging, licensing, platform limits and measured accuracy/latency are documented and
      the provider passes X-105 plus the full gate.

## Progress

- Backlog. Depends on A-25, A-28 and M-54.
