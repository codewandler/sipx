---
id: A-26
title: Emit speech-recognition events through the application SDK
pillar: Application
status: backlog
priority: 25
design: docs/designs/local-speech.md
epic: local-speech
areas: [app-sdk, speech, recognition, m16]
predicate:
announcement:
note: after C-3, C-5, A-25, M-54 and M-55 · ordered utterance and provider lifecycle
---

# Emit speech-recognition events through the application SDK

## Goal

Deliver recognition results and lifecycle through the typed call event stream so applications do
not poll or depend on a concrete provider.

## Acceptance

- [ ] Typed events cover provider selection, warm-up, ready, partial, replacement, final,
      cancellation, fallback, loss and failure with call, provider and utterance identity.
- [ ] A failing-first test proves partial/replacement/final ordering, exactly one terminal outcome
      per utterance and deterministic behavior across media discontinuities.
- [ ] The SDK can start, stop and cancel recognition per call and select a compatible provider
      override without exposing implementation handles.
- [ ] A full event queue follows documented bounded coalescing/drop behavior without blocking RTP;
      final, cancellation and failure outcomes cannot be silently lost.
- [ ] Recognition/provider failure stays distinct from SIP call failure, and call cancellation
      drains provider work before the SDK reports completion.
- [ ] Generated bindings, interpreter vectors, API docs and the full gate are green.

## Progress

- Backlog. Depends on C-3, C-5, A-25, M-54 and M-55.
