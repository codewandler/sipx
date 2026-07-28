---
id: T-9
title: Implement secure WebSocket
pillar: Signalling
status: done
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
- [x] WSS composes the TLS work from `T-7` with the WebSocket work from `T-8` rather than
      duplicating either.
- [x] Certificate verification is the same code and the same policy as `T-7`; a second,
      subtly different implementation is how one of them ends up weaker.
- [x] Failing-first test: `a_call_establishes_over_wss`.

## Progress
- Done. WSS is `T-7`'s `ServerTls`/`ClientTls` with `T-8`'s framing on top — the same acceptor,
  the same connector, the same policy — rather than a second implementation of either.
- Tests in `crates/sipx-transport/tests/wss.rs` (transport) and `crates/sipx-call/tests/wss.rs`
  (`a_call_establishes_over_wss`: INVITE, 200, ACK, audio, BYE, all signalling inside TLS).
- Found and fixed a `T-7` hole this exposed: RFC 3263 resolution never attached the URI host to
  a candidate, so a `sips:` URI resolved through NAPTR/SRV had its certificate checked against
  the *resolved address*. That is exactly what `sip-tls.md` §3.3 exists to prevent — the
  handshake still succeeds and the check becomes decorative.
