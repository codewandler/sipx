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
      NOTIFY, an unsupported package, shutdown with a refresh due, provisional expiry, CSeq
      exhaustion, invalid interval responses and NOTIFY trust/Contact rejection.
- [ ] `S-38` acceptance and tests cite those vectors, and `./scripts/gate.py` is green.

## Progress

- 2026-08-05: [`docs/specs/event-client.md`](../specs/event-client.md) fixes the sans-I/O boundary,
  bounded resources/timers, initial-NOTIFY/dialog race, single-dialog fork policy, auth and interval
  retries, CSeq ordering, terminal-reason policy, shutdown drain and the initial eight byte-level
  vectors. S-38 names the failing-first test derived from each vector. The story remains in progress
  until the integration gate runs.
- 2026-08-05: independent review added a finite pre-response expiry, exact invalid-response
  transitions, non-wrapping CSeq exhaustion, mandatory Contact validation and a fail-closed injected
  NOTIFY trust policy; S-38 now maps four additional negative vectors.
- 2026-08-05: follow-up review fixed refresh Timer N: it ends only the refresh attempt, never moves
  the authoritative Expiry, never schedules an implicit retry and has its own S37-V13 proof.
