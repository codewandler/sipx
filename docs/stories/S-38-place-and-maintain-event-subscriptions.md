---
id: S-38
title: Place and maintain event subscriptions
pillar: Signalling
status: backlog
priority: 7
design: docs/designs/event-reachability.md
epic: event-reachability
areas: [sipx-ua, sipx-transport, m13, parity-wave-1]
predicate:
announcement:
note: after S-37 · reusable outbound SUBSCRIBE and NOTIFY tracking · S-24 is a consumer
---

# Place and maintain event subscriptions

## Goal

Expose the reusable endpoint path that issues SUBSCRIBE and maintains the resulting notification
dialog, without embedding any event package's application policy in the transport machinery.

## Acceptance

- [ ] The public API establishes, refreshes and terminates a subscription through the state machine
      and byte vectors specified by `S-37`.
- [ ] 401 and 407 challenges reuse endpoint credentials; refreshes use the granted expiry; initial
      and subsequent NOTIFY requests are ordered and surfaced with typed subscription state.
- [ ] The dialog remote target, route set and CSeq rules are honored for every request, and a
      terminated or rejected subscription cannot be silently resurrected.
- [ ] Live subscriptions, pending notification delivery and refresh timers are bounded. Cancellation
      waits for owned work and a test observes zero residual transactions and timers.
- [ ] A synthetic package proves the generic API; `S-24` consumes it for `reg` without copying the
      subscriber state machine.
- [ ] RFC registry evidence is updated with reachable Rust paths and `./scripts/gate.py` is green.

## Progress

- Not started. Depends on S-37; may run beside S-35 once the spec is accepted.
