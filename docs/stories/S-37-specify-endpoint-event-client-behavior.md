---
id: S-37
title: Specify endpoint event-client behavior
pillar: Signalling
status: in-progress
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

- [x] A spec in `docs/specs/` cites RFC 3261, RFC 3265 and RFC 6665 and defines dialog creation,
      authentication, refresh, expiry, termination and NOTIFY ordering as state tables.
- [x] The contract defines initial-NOTIFY races, monotonically changing local CSeq, remote CSeq
      validation, `Subscription-State` reasons, retry rules and the deliberate refusal of forking.
- [x] Timer and resource tables state maximum live subscriptions, queued notifications, refresh work
      and shutdown behavior; time enters the core only as fired timer input.
- [x] Event-package payload interpretation is an injected consumer. The generic client never imports
      discovery, presence UI or call policy.
- [x] Byte-level vectors cover authenticated establishment, refresh, expiry, unsubscribe, out-of-order
      NOTIFY, an unsupported package and shutdown with a refresh due.
- [ ] `S-38` acceptance and tests cite those vectors, and `./scripts/gate.py` is green.

## Progress

- 2026-08-05: [`docs/specs/event-client.md`](../specs/event-client.md) fixes the sans-I/O boundary,
  bounded resources/timers, initial-NOTIFY/dialog race, single-dialog fork policy, auth and interval
  retries, CSeq ordering, terminal-reason policy, shutdown drain and eight byte-level vectors. S-38
  now names the failing-first test derived from each vector. The story remains in progress until the
  integration gate runs.
