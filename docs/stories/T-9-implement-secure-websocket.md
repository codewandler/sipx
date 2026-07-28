---
id: T-9
title: Implement secure WebSocket
pillar: Signalling
status: ready
priority: 4
design: docs/designs/sip-transport.md
epic: depth
areas: [sipx-transport]
note:
---

# Implement secure WebSocket

## Goal
WSS, which is the only WebSocket transport a browser will use to a page served over HTTPS.

## Acceptance
- [ ] WSS composes the TLS work from `T-7` with the WebSocket work from `T-8` rather than
      duplicating either.
- [ ] Certificate verification is the same code and the same policy as `T-7`; a second,
      subtly different implementation is how one of them ends up weaker.
- [ ] Failing-first test: `a_call_establishes_over_wss`.

## Progress
- Not started.
