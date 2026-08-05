---
id: A-31
title: Emit typed Realtime understanding and transcript events
pillar: Application
status: backlog
priority: 33
design: docs/designs/openai-realtime-phone.md
epic: openai-realtime-phone
areas: [testkit, app-sdk, transcript, openai-realtime, m17]
predicate:
announcement:
note: after A-30 and C-3 · model output is untrusted application data, never caller authorization
---

# Emit typed Realtime understanding and transcript events

## Goal

Expose what the model heard and inferred through the application SDK with ordering and provenance,
without treating model output as authenticated caller intent.

## Acceptance

- [ ] Typed events cover input-speech start/stop, transcript partial/replacement/final,
      understanding/item completion, cancellation and error with call/session/item/utterance identity.
- [ ] A failing-first sequence proves partial replacement, finalization, response cancellation,
      reconnect replacement and exactly one terminal outcome per item.
- [ ] Events identify external-model provenance and carry no authority bit; documentation states that
      transcript or understanding alone cannot authorize a consequential phone action.
- [ ] Event queues are bounded and use a documented coalescing/drop policy that preserves final,
      cancellation and error state without blocking media or the Realtime session.
- [ ] Caller content is absent from ordinary diagnostics unless explicit application policy opts in,
      and concurrent calls cannot receive each other's transcript or understanding events.
- [ ] Generated bindings, interpreter vectors, public API docs and the full gate are green.

## Progress

- Backlog. Depends on A-30 and C-3.
