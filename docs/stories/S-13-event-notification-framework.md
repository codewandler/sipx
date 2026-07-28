---
id: S-13
title: Build the event notification framework
pillar: Signalling
status: backlog
priority: 8
design:
epic: conformance
areas: [sipx-ua, sipx-sip]
note: RFC 6665; generalises what REFER already does implicitly
---

# Build the event notification framework

## Goal
SUBSCRIBE and NOTIFY as a framework with pluggable packages, rather than the single implicit
subscription REFER creates today.

## Acceptance
- [ ] SUBSCRIBE establishes a subscription with its own dialog, refresh and expiry.
- [ ] NOTIFY carries `Subscription-State`, and a terminated subscription is not resurrected by a
      later notification.
- [ ] Packages register by name; an unknown `Event` is refused with 489 rather than accepted.
- [ ] The implicit subscription from `S-9` is expressed in terms of this rather than beside it.
- [ ] `Refer-Sub: false` (RFC 4488) suppresses it when the transferor does not want it.
- [ ] Failing-first test: `a_terminated_subscription_stops_notifying`.

## Progress
- Not started. RFC 6665 is `partial` in `compliance.md`: the implicit subscription is
  implemented, the framework is not.

## Notes
- Gates the registration, dialog and presence packages — everything a busy-lamp field needs.
