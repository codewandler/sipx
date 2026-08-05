---
id: S-35
title: Accept an inbound subscription from a socket
pillar: Signalling
status: done
priority: 2
design: docs/designs/event-reachability.md
epic: event-reachability
areas: [sipx-ua, sipx-call, m13, parity-wave-1]
predicate:
announcement:
note: RFC 6665 notifier is reachable from a live endpoint and shares the bounded subscription store
---

# Accept an inbound subscription from a socket

## Goal

Give the implemented RFC 6665 notifier a caller: route an inbound SUBSCRIBE from the dispatcher into
the existing subscription store, so the three shipped event packages become reachable rather than
library-only.

## Acceptance

- [x] The dispatcher serves SUBSCRIBE alongside the methods it already answers, routing an accepted
      subscription into the store in `crates/sipx-ua/src/subscribe.rs`. **No second store is
      introduced** — a failing-first test proves the socket path and the library API observe the same
      subscription.
- [x] The initial NOTIFY required by RFC 6665 §4.1.2 is sent immediately on acceptance. This is the
      story's primary failing-first test: a subscription accepted and then silent looks healthy on the
      wire and is not.
- [x] Refusals stay explicit and typed: an unserved package is 489 as today, a subscription to a
      dialog that does not exist is 481, and an expiry outside the served range is negotiated down per
      RFC 6665 §4.2.1 rather than silently accepted.
- [x] Concurrent subscriptions and per-package state are bounded by construction, matching
      `docs/designs/bounded-transports.md`. A test drives the bound and asserts the shed is visible —
      counter, log or response — per the rule in `crates/sipx-transport/tests/backpressure.rs`.
- [x] Terminating a subscription stops its timers, proven by observing timer and task termination
      rather than only that state was removed.
- [x] The subscriber half — issuing SUBSCRIBE, tracking `Subscription-State` — is **out of scope**
      and is not partially built here.
- [x] `docs/rfc/registry.toml` RFC 6665 row and the affected package rows (4235, 3680, 3856) are
      updated in the same commit; `rfc-report.py --check` green.
- [x] `./scripts/gate.py` green, including `check-fixed-sleep.py`.

## Progress
- 2026-08-05: socket-driver contract is being specified before the dispatcher and runtime change.
- 2026-08-05: notifier runtime, loopback proofs, RFC evidence and public docs are complete.
- 2026-08-05: independent review made header parsing fail closed, narrowed runtime package
  admission to the three rendered tokens, rejected untagged identity collisions, bounded each
  NOTIFY transaction, and added paused-time expiry plus all-package wire proofs.
- 2026-08-05: second independent review specified replay-safe remote SUBSCRIBE sequencing,
  RFC-defined Event/type identity, case-sensitive opaque dialog tags and duplicate-header refusal;
  focused implementation and regression evidence passed, followed by all 36 integration-gate steps.

## Notes
- The sharpest instance of the pattern the 2026-08-04 capability review named as sipx's real feature
  gap: the code is not missing, the caller is. `X-37` already settled the doctrine — reachability is
  resolved through callers.
- Unblocks `S-24`, which needs both this and a subscriber.
- First path in `sipx-ua` where a remote party makes sipx originate traffic on a schedule. Treat the
  bound and the shutdown path with the seriousness the transport layer gets.
