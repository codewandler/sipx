---
id: A-27
title: Control synthesized call speech through the application SDK
pillar: Application
status: backlog
priority: 26
design: docs/designs/local-speech.md
epic: local-speech
areas: [app-sdk, speech, synthesis, audio-analysis, m16]
predicate:
announcement:
note: after A-25, M-17, M-56 and M-58 · bounded playback, cancellation and activity-aware ducking
---

# Control synthesized call speech through the application SDK

## Goal

Let an application enqueue, play, cancel and optionally duck generated speech on a live call while
observing every accepted and terminal transition through the typed SDK.

## Acceptance

- [ ] Bounded SDK commands cover enqueue, play, cancel and gain/duck policy with call, provider and
      request identity; the provider never receives an unrestricted call handle.
- [ ] Typed events cover accepted, started, completed, cancelled, ducked, resumed, fallback and
      failed outcomes with exactly one terminal event per request.
- [ ] A failing-first test proves FIFO and explicit replacement policy, interrupt cancellation,
      queue-full refusal and teardown with no leaked audio after hangup.
- [ ] Optional activity-aware ducking consumes M-58's typed state and uses bounded gain policy; it
      does not implement SIP hold or mute and can be disabled per call.
- [ ] Provider loss and explicit fallback remain distinct from call failure and never silently
      change voice, language, privacy or quality policy.
- [ ] Generated bindings, interpreter vectors, API docs and the full gate are green.

## Progress

- Backlog. Depends on A-25, M-17, M-56 and M-58.
