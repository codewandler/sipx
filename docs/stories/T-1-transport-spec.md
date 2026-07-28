---
id: T-1
title: Specify the transport layer and the sans-IO driver contract
pillar: Signalling
status: done
priority:
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport]
note: gates every other transport story
---

# Specify the transport layer and the sans-IO driver contract

## Goal
Define exactly how the async layer drives the sans-IO core — the input and output vocabulary,
who owns timers, and how connections map to transactions — before any socket code is written.

## Acceptance
- [x] `docs/specs/sip-transport.md` defines the `Input`/`Output` vocabulary, timer ownership,
      and the backpressure and shutdown model.
- [x] Specifies per-transport behaviour: datagram vs. stream framing, when a connection is
      reused (RFC 5923), and what happens to in-flight transactions when a connection drops.
- [x] Specifies NAT handling: `rport` (RFC 3581), `received`, and sent-by rewriting, each with
      the condition under which it applies.
- [x] Specifies RFC 3263 resolution order — NAPTR, then SRV, then A/AAAA — and the fallbacks
      when records are missing, including transport selection for `sips:`.
- [x] Records the decision on connection reuse defaults and its security implications.

## Progress
- Done. `docs/specs/sip-transport.md`.
- The connection-reuse default is settled: an inbound connection is reused for that
  transaction's responses always, and **not** for unrelated outbound requests unless
  configured. Reusing it is convenient and is also how a peer that connected to you gets your
  outbound traffic routed through it.
- Backpressure is settled too: when the application queue fills, new server transactions are
  answered 503 with `Retry-After`. Blocking the loop would stop timers, turning a slow
  application into a stack that drops calls it had already established; dropping events
  silently loses requests. 503 is the one option that tells the peer something true.

## Notes
- This spec is what keeps the sans-IO boundary honest; if it can't be written cleanly, the
  boundary is in the wrong place.
