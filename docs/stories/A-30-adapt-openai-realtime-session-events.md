---
id: A-30
title: Adapt OpenAI Realtime session and lifecycle events
pillar: Application
status: backlog
priority: 32
design: docs/designs/openai-realtime-phone.md
epic: openai-realtime-phone
areas: [testkit, app-sdk, websocket, openai-realtime, m17]
predicate:
announcement:
note: after A-22 · extend the delivered bridge with correlated lifecycle and deliberate replacement
---

# Adapt OpenAI Realtime session and lifecycle events

## Goal

Map the event-driven WebSocket protocol into one bounded, finite, call-owned state machine with
typed lifecycle, correlation and cleanup behavior.

## Acceptance

- [ ] A normative state table covers connect, session created/updated, input speech, transcript,
      audio, response, rate-limit, error, cancellation, close and terminal call teardown events.
- [ ] Every client event has a generated correlation ID where the protocol permits one, and server
      errors, function items, output chunks and completions are tied to the correct request/item.
- [ ] Session age is bounded below the documented service maximum; expiry ends or deliberately
      replaces the session through observable state rather than silently losing the call attachment.
- [ ] A transport loss may create a fresh session only under explicit application policy; pending
      audio, conversation state and action requests are not reported as resumed or replayed.
- [ ] Interruption extends A-22's bounded playback accounting to order response cancellation,
      playback stop and unplayed-audio truncation, with deterministic tests for every race around
      response completion.
- [ ] Malformed, unknown, duplicate, late and out-of-order events cannot panic or grow state without
      bound; cancellation drains the socket/session task and the full gate is green.

## Progress

- Backlog. Extends the delivered A-22 bridge; it does not create a second audio path or session
  configuration surface.
