---
id: T-32
title: Expose bounded endpoint observation and policy hooks
pillar: Transport
status: backlog
priority: 9
design: docs/designs/live-endpoint-policy.md
epic: live-endpoint-policy
areas: [sipx-transport, sipx-ua, security, m13, parity-wave-1]
predicate:
announcement:
note: typed lifecycle seams · bounded observation · no arbitrary post-key message mutation
---

# Expose bounded endpoint observation and policy hooks

## Goal

Give hosts safe visibility and a narrow policy seam at named endpoint lifecycle points without
letting callbacks stall the driver or corrupt transaction and authentication invariants.

## Acceptance

- [ ] A read-only event surface observes parsed inbound and finalized outbound messages with source,
      target, transport and transaction classification.
- [ ] The same surface reports connection accepted/opened, authenticated, pooled/reused, failed and
      closed transitions with stable typed identifiers.
- [ ] Delivery is bounded and never awaits application work from the endpoint driver; overflow is
      counted and observable, and observer closure or failure cannot stop network processing.
- [ ] A separate pre-transaction policy may approve, reject or add application-owned headers before
      branch, transaction key, Digest, Via and Content-Length are finalized.
- [ ] A source-admission policy runs before SIP parsing for UDP datagrams and before TLS/WebSocket
      handshake or stream parsing for connection transports. The host can atomically replace or
      clear a bounded IP/prefix allowlist without rebinding; existing admitted connections retain a
      stated generation policy, and refused sources are counted without allocating per-source work.
- [ ] No post-key mutator can rewrite Call-ID, CSeq, route set, branch or authenticated bytes. Target
      selection continues through the existing resolver and explicit target APIs.
- [ ] Capture and counters remain the zero-custom-code observation path and are not reimplemented by
      the hook.
- [ ] Saturation, closed-consumer, protected-field mutation, UDP source refusal, pre-handshake
      connection refusal and live allowlist replacement tests fail first, then pass; the full gate
      is green.

## Progress

- Not started. Follows T-31 so the epic has one reviewed live-update doctrine before adding policy.
  X-97 review added the pre-parse source-admission contract; parsed-message hooks alone do not close
  the transport-whitelist leaf.
