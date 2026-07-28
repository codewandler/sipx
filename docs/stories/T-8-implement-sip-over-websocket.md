---
id: T-8
title: Implement SIP over WebSocket
pillar: Signalling
status: ready
priority: 3
design: docs/designs/sip-transport.md
epic: depth
areas: [sipx-transport]
note:
---

# Implement SIP over WebSocket

## Goal
SIP over WebSocket (RFC 7118), which is how a browser reaches a SIP network at all.

## Acceptance
- [ ] The handshake negotiates the `sip` subprotocol and refuses a peer that does not offer it.
- [ ] One SIP message per WebSocket frame, per RFC 7118 §5 — not `Content-Length` framing.
- [ ] A `Via` naming a WebSocket hop is understood, and responses return over the same
      connection, since a browser has no listening port and can never be connected back to.
- [ ] Ping/pong keeps the connection alive through intermediaries that time out idle sockets.
- [ ] Failing-first test: `a_message_arrives_as_one_websocket_frame`.

## Progress
- Not started.
