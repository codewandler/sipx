---
id: T-8
title: Implement SIP over WebSocket
pillar: Signalling
status: done
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
- [x] The handshake negotiates the `sip` subprotocol and refuses a peer that does not offer it.
- [x] One SIP message per WebSocket frame, per RFC 7118 §5 — not `Content-Length` framing.
- [x] A `Via` naming a WebSocket hop is understood, and responses return over the same
      connection, since a browser has no listening port and can never be connected back to.
- [x] Ping/pong keeps the connection alive through intermediaries that time out idle sockets.
- [x] Failing-first test: `a_message_arrives_as_one_websocket_frame`.

## Progress
- Done. `crates/sipx-transport/src/ws.rs`, driven from the pool in `tcp.rs` and the endpoint
  loop; tests in `crates/sipx-transport/tests/ws.rs` against a bare WebSocket rather than a
  second sipx, because framing bugs hide when both ends share a framer.
- The connection pool is now keyed by `(address, transport, verified identity)` rather than by
  address alone. WebSocket forced the question — WS and TCP can share a port — and it also
  closes the `sip-tls.md` §5 gap that `T-7` left open (vector L10).
- `Contact` and the in-dialog target turned out to be part of this, not of the call layer: a
  WebSocket client that advertises a real address has every ACK and BYE aimed at a port that
  is not listening. See `sip-tls.md` §4.
- Fixed along the way: the crate did not build with `tls` disabled, because `tokio::select!`
  cannot compile a branch out behind a feature flag. All optional listeners now share one
  channel and one branch, and every feature combination is checked.
