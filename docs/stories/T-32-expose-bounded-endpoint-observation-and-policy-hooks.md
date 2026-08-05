---
id: T-32
title: Expose bounded endpoint observation and policy hooks
pillar: Transport
status: done
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

- [x] A read-only event surface observes parsed inbound and finalized outbound messages with source,
      target, transport and transaction classification.
- [x] The same surface reports connection accepted/opened, authenticated, pooled/reused, failed and
      closed transitions with stable typed identifiers.
- [x] Delivery is bounded and never awaits application work from the endpoint driver; overflow is
      counted and observable, and observer closure or failure cannot stop network processing.
- [x] A separate pre-transaction policy may approve, reject or add application-owned headers before
      branch, transaction key, Digest, Via and Content-Length are finalized.
- [x] No post-key mutator can rewrite Call-ID, CSeq, route set, branch or authenticated bytes. Target
      selection continues through the existing resolver and explicit target APIs.
- [x] A live IP/prefix source-admission set is checked before UDP parsing and before TLS/WebSocket
      handshaking or stream parsing. Replace and clear publish atomically; an accepted connection
      retains the generation that admitted it until close, and refusals are counted without
      per-source tasks, futures or state. A validated non-zero configured maximum bounds scan work;
      oversized replacement is typed and preserves the old generation.
- [x] Capture and counters remain the zero-custom-code observation path and are not reimplemented by
      the hook.
- [x] Saturation, closed-consumer, protected-field mutation, UDP source refusal, pre-handshake
      connection refusal and live allowlist replacement tests fail first, then pass; the full gate
      is green.

## Progress

- Completed after T-31 review. The source-admission correction discovered by X-97 is part of this
  story rather than hidden inside the broader message-policy row. The corrected integration gate
  passed all 36 steps.
