---
id: X-107
title: Prove the Realtime phone against mock and opt-in live services
pillar: Build
status: backlog
priority: 35
design: docs/designs/openai-realtime-phone.md
epic: openai-realtime-phone
areas: [testkit, integration-test, openai-realtime, security, m17]
predicate:
announcement:
note: after A-30 through A-33 · extend A-21 deterministic CI and A-23's bounded live proof
---

# Prove the Realtime phone against mock and opt-in live services

## Goal

Test every protocol, audio, interruption and action-policy path deterministically, then provide a
separate explicitly requested live proof using `OPENAI_API_KEY` without making CI depend on it.

## Acceptance

- [ ] A bounded local service double scripts session, speech, transcript, audio, response,
      function-call, rate-limit and error events with exact ordering and correlation.
- [ ] Deterministic tests cover chunking/backpressure, malformed and duplicate events, disconnect,
      finite-session replacement, response cancellation, playback stop and conversation truncation.
- [ ] Action tests prove requested/accepted/refused/completed sequences, schema rejection,
      idempotency, timeouts, confirmation and no phone mutation from an unadvertised call.
- [ ] The live arm extends A-23's one-call credential guard rather than creating a second live-test
      authority; absence is disclaimed, never reported as live success and never weakens
      deterministic CI.
- [ ] Live-test artifacts redact the key and caller content, enforce session/usage/cost/time bounds,
      supervise every process and close the phone, media and WebSocket resources on every exit.
- [ ] Both test paths use the packaged integration surface, document prerequisites and have focused
      checks plus the full gate green.

## Progress

- Backlog. M17 conformance proof after the implementation stories, reusing A-21 and A-23.
