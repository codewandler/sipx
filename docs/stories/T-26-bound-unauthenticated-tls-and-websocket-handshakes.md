---
id: T-26
title: Bound unauthenticated TLS and WebSocket handshakes
pillar: Signalling
status: ready
priority: 2
design: docs/designs/bounded-transports.md
epic: bounded-transports
areas: [sipx-transport]
predicate: 4
note: R-02 in the 2026-07-30 repository review — accepted stream handshakes are spawned before pool admission with no deadline or concurrency budget
---

# Bound unauthenticated TLS and WebSocket handshakes

## Goal

Put incomplete inbound TLS and WebSocket handshakes under explicit time and concurrency budgets so an
unauthenticated peer cannot create unbounded pre-pool tasks and sockets.

## Acceptance

- [ ] Add the handshake admission, deadline and shutdown state to `docs/specs/sip-transport.md`,
      including TLS, WebSocket and secure WebSocket listeners.
- [ ] An accepted stream acquires a bounded handshake permit before a handshake task is spawned; when
      no permit is available, the endpoint applies one documented refusal policy without growing an
      unbounded wait queue.
- [ ] Partial handshakes time out, close their sockets and release their permits. Failure, success and
      endpoint shutdown also release exactly one permit.
- [ ] The public configuration has safe non-zero defaults and is validated before listener tasks
      start.
- [ ] Failing-first tests hold more partial handshakes than the configured limit, prove the number of
      live handshake tasks never exceeds it, advance past the deadline, and prove all permits and
      sockets are reclaimed.
- [ ] A focused unauthenticated connection-flood test remains responsive and within the configured
      live-handshake budget.

## Progress

- Filed from R-02 in `docs/reviews/2026-07-30T07-50-49+02-00-repository-review.md`.
- No existing story covers the pre-pool lifetime. T-7 and T-17 implement the secure transports, while
  T-3 bounds only admitted pooled connections.

## Notes

- The resource limit must cover live tasks and sockets, not only handshakes that have completed far
  enough to obtain a connection-pool key.
