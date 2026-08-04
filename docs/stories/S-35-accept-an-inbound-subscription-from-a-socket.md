---
id: S-35
title: Accept an inbound subscription from a socket
pillar: Signalling
status: backlog
priority: 17
design: docs/designs/event-reachability.md
epic: event-reachability
areas: [sipx-ua, sipx-call]
predicate:
announcement:
note: RFC 6665 notifier is implemented and unreachable · nothing in the workspace receives a SUBSCRIBE · unblocks S-24 · follow-up
---

# Accept an inbound subscription from a socket

## Goal

Give the implemented RFC 6665 notifier a caller: route an inbound SUBSCRIBE from the dispatcher into
the existing subscription store, so the three shipped event packages become reachable rather than
library-only.

## Acceptance

- [ ] The dispatcher serves SUBSCRIBE alongside the methods it already answers, routing an accepted
      subscription into the store in `crates/sipx-ua/src/subscribe.rs`. **No second store is
      introduced** — a failing-first test proves the socket path and the library API observe the same
      subscription.
- [ ] The initial NOTIFY required by RFC 6665 §4.1.2 is sent immediately on acceptance. This is the
      story's primary failing-first test: a subscription accepted and then silent looks healthy on the
      wire and is not.
- [ ] Refusals stay explicit and typed: an unserved package is 489 as today, a subscription to a
      dialog that does not exist is 481, and an expiry outside the served range is negotiated down per
      RFC 6665 §4.2.1 rather than silently accepted.
- [ ] Concurrent subscriptions and per-package state are bounded by construction, matching
      `docs/designs/bounded-transports.md`. A test drives the bound and asserts the shed is visible —
      counter, log or response — per the rule in `crates/sipx-transport/tests/backpressure.rs`.
- [ ] Terminating a subscription stops its timers, proven by observing timer and task termination
      rather than only that state was removed.
- [ ] The subscriber half — issuing SUBSCRIBE, tracking `Subscription-State` — is **out of scope**
      and is not partially built here.
- [ ] `docs/rfc/registry.toml` RFC 6665 row and the affected package rows (4235, 3680, 3856) are
      updated in the same commit; `rfc-report.py --check` green.
- [ ] `./scripts/gate.py` green, including `check-fixed-sleep.py`.

## Progress
- (not started)

## Notes
- The sharpest instance of the pattern the 2026-08-04 capability review named as sipx's real feature
  gap: the code is not missing, the caller is. `X-37` already settled the doctrine — reachability is
  resolved through callers.
- Unblocks `S-24`, which needs both this and a subscriber.
- First path in `sipx-ua` where a remote party makes sipx originate traffic on a schedule. Treat the
  bound and the shutdown path with the seriousness the transport layer gets.
