# Design: Transport layer

**Status:** outline — filled in by `T-1` · **Pillar:** Signalling · **Epic:** `sip-transport` ·
**Stories:** T-1 … T-4

## Why

The transport layer is the only place in the signalling stack that touches the network, which
makes it the only place that can get NAT, connection lifetime and target resolution right — or
wrong. It is also the driver for the sans-IO core, so its contract determines whether the
core's testability survives contact with a real runtime.

## Approach

_To be written by `T-1`, which produces `docs/specs/sip-transport.md`. In outline: an async
driver owning sockets and a timer wheel, feeding `Input`s to `sipx-sip` and executing its
`Output`s; a connection pool keyed by (transport, remote); RFC 3263 resolution behind an
injectable resolver; per-transport feature flags so a UDP-only build carries no TLS or
WebSocket code._

## Alternatives considered

- _Pending `T-1`._

## Risks & open questions

- Connection reuse (RFC 5923) is a security-relevant default: reusing an inbound connection
  for outbound requests is convenient and is also how a malicious peer gets requests routed
  through it. The default must be decided deliberately in `T-1`.
- Backpressure: what happens when the application cannot consume events as fast as the
  network delivers them. Dropping and blocking are both wrong in different ways.

## Acceptance / done

The union of `T-1`…`T-4`: messages sent and received over each enabled transport, `rport`
handled, connections pooled and reused, targets resolved per RFC 3263, and a loopback harness
proving the core is driven correctly.
