---
id: S-39
title: Send and receive publications through an endpoint
pillar: Signalling
status: in-progress
priority: 8
design: docs/designs/event-reachability.md
epic: event-reachability
areas: [sipx-ua, sipx-sip, m13, parity-wave-1]
predicate:
announcement:
note: after S-37 · make the existing RFC 3903 compositor and entity tags wire-reachable
---

# Send and receive publications through an endpoint

## Goal

Carry the existing publication compositor and entity-tag lifecycle through live inbound and outbound
PUBLISH paths without introducing another presence store.

## Acceptance

- [x] The dispatcher routes inbound PUBLISH to the compositor shipped by `S-18`, and 200, 412 and
      423-class outcomes plus the current `SIP-ETag` leave on the wire.
- [x] A public publisher creates, refreshes, modifies and removes publication state while retaining
      the latest entity tag and granted expiry across requests.
- [x] 401/407 retry, stale-tag recovery and conditional update behavior follow RFC 3903 and return
      typed errors when the application must republish from scratch.
- [x] Publication bodies, active publications and refresh timers are bounded; cancellation drains
      owned work and a failing-first test observes no residual publication or transaction.
- [x] Composition policy, authorization policy and durable or distributed storage remain injected
      application concerns.
- [ ] RFC 3903 registry evidence moves to the reachable endpoint paths and `./scripts/gate.py` is
      green.

## Progress

- Added the normative publication endpoint contract, typed PUBLISH headers and a bounded sans-I/O
  publisher with digest/interval retry, conditional state, exact response validation and finite
  timers.
- Added dispatcher-owned inbound and outbound live endpoint paths. Wire tests cover create,
  refresh, modify, remove, stale cross-resource tags, authentication, timer refresh and zero
  residual publication/transaction work.
- Integration still owns the complete gate and generated board before marking this story done.
