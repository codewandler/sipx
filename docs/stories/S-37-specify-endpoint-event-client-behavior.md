---
id: S-37
title: Specify endpoint event-client behavior
pillar: Signalling
status: ready
priority: 3
design: docs/designs/event-reachability.md
epic: event-reachability
areas: [sipx-sip, sipx-ua, docs, m13, parity-wave-1]
predicate:
announcement:
note: spec before code · generic RFC 6665 client contract consumed by S-24
---

# Specify endpoint event-client behavior

## Goal

Write the normative, sans-I/O contract for an endpoint that originates and maintains subscriptions
before the transport-facing client is implemented.

## Acceptance

- [ ] A spec in `docs/specs/` cites RFC 3261, RFC 3265 and RFC 6665 and defines dialog creation,
      authentication, refresh, expiry, termination and NOTIFY ordering as state tables.
- [ ] The contract defines initial-NOTIFY races, monotonically changing local CSeq, remote CSeq
      validation, `Subscription-State` reasons, retry rules and the deliberate refusal of forking.
- [ ] Timer and resource tables state maximum live subscriptions, queued notifications, refresh work
      and shutdown behavior; time enters the core only as fired timer input.
- [ ] Event-package payload interpretation is an injected consumer. The generic client never imports
      discovery, presence UI or call policy.
- [ ] Byte-level vectors cover authenticated establishment, refresh, expiry, unsubscribe, out-of-order
      NOTIFY, an unsupported package and shutdown with a refresh due.
- [ ] `S-38` acceptance and tests cite those vectors, and `./scripts/gate.py` is green.

## Progress

- Not started. Runs beside X-97.
