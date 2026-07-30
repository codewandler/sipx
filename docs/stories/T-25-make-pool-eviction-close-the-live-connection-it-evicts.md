---
id: T-25
title: Make pool eviction close the live connection it evicts
pillar: Signalling
status: done
priority: 1
design: docs/designs/bounded-transports.md
epic: bounded-transports
areas: [sipx-transport]
predicate: 4
note: R-01 in the 2026-07-30 repository review — map removal drops the writer but leaves the socket task blocked on reads, so the configured pool bound is not a live-connection bound
---

# Make pool eviction close the live connection it evicts

## Goal

Make idle and capacity eviction terminate the corresponding transport task and socket, so the
configured connection-pool maximum bounds live connections rather than only map entries.

## Acceptance

- [ ] Specify the lifecycle and cancellation path for pooled TCP, TLS, WebSocket and secure WebSocket
      connections in `docs/specs/sip-transport.md` before changing the implementation.
- [ ] Idle and LRU eviction signal the connection task to stop; a quiet peer cannot keep the task alive
      by leaving its read half open after every writer sender has gone away.
- [ ] Endpoint shutdown uses the same ownership path and does not detach evicted connection tasks.
- [ ] Pool metrics and capacity decisions count every live pooled connection until its task has
      terminated, or document and enforce an equally strong reserved-slot invariant.
- [ ] Failing-first tests observe peer EOF and task completion after both idle and capacity eviction
      for a byte-stream transport and a WebSocket transport. A map-length assertion alone is not
      sufficient.
- [ ] Run a focused connection-churn test demonstrating that live task and socket counts stay bounded
      while quiet peers are repeatedly evicted.

## Progress

- Filed from R-01 in `docs/reviews/2026-07-30T07-50-49+02-00-repository-review.md`.
- This is a defect against completed story T-3's bounded-pool acceptance, not a duplicate of an
  existing active story. T-3 remains the historical implementation record; this story tracks the
  concrete broken invariant.

## Notes

- Review evidence points to `crates/sipx-transport/src/tcp.rs` and
  `crates/sipx-transport/src/ws.rs`: channel closure disables a refutable receive branch while socket
  reads or WebSocket pings keep the task alive.
