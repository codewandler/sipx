---
id: S-13
title: Build the event notification framework
pillar: Signalling
status: done
priority:
design:
epic: conformance
areas: [sipx-ua, sipx-sip]
note: M8 · RFC 6665 · large; the other two packages are on it
---

# Build the event notification framework

## Goal
SUBSCRIBE and NOTIFY as a framework with pluggable packages, rather than the single implicit
subscription REFER creates today.

## Acceptance
- [x] SUBSCRIBE establishes a subscription with its own dialog, refresh and expiry.
- [x] NOTIFY carries `Subscription-State`, and a terminated subscription is not resurrected by a
      later notification.
- [x] Packages register by name; an unknown `Event` is refused with 489 rather than accepted.
- [x] The implicit subscription from `S-9` is expressed in terms of this rather than beside it.
- [x] `Refer-Sub: false` (RFC 4488) suppresses it when the transferor does not want it.
- [x] Failing-first test: `a_terminated_subscription_stops_notifying`.

## Progress
- Done, split the usual way. `sipx-sip/src/event.rs` decides — what a `Subscription-State` says,
  which reasons mean "try again", what expiry a notifier may grant, whether a package is served —
  and `sipx-ua/src/subscribe.rs` holds the subscriptions that exist, taking `now` as an argument
  rather than reading a clock.
- **A terminated subscription stays terminated**, which is the failing-first test and the property
  everything else hangs off. `notify_state` returns `None` for one, so there is no way to produce
  an `active` state for a subscription that has ended; and a refresh arriving afterwards is refused
  rather than treated as a new subscription in the same dialog. §4.1.3 makes termination final, and
  a subscriber that wants another one starts a new dialog.
- **Terminating is not forgetting.** A terminated subscription stays findable until `sweep`, so a
  NOTIFY that crosses it on the wire finds a subscription that is over rather than one that never
  existed — which are different things to a subscriber.
- **The identity is the dialog *and* the package.** §4.4.1 allows several subscriptions in one
  dialog as long as their `Event` differs, so keying on the dialog alone lets a second subscription
  silently replace the first.
- 489 for an unserved package, by name, rather than accepting and never notifying — which a
  subscriber cannot tell from a slow notifier. Not 400 (malformed) and not 501 (method), both of
  which would mislead it about whether to retry.
- **The implicit subscription is now expressed in the framework's terms**: `transfer::is_terminated`
  asks `event::Subscription` instead of parsing `Subscription-State` a second time. Two parsers for
  one header eventually disagree about whether a transfer has finished.
- **`Refer-Sub: false` (RFC 4488) needs both sides.** §3 has the transferor *request* suppression
  and the transferee *agree* by echoing it; a transferor that assumed agreement would stop watching
  for notifications the transferee is still sending. `Refer-Sub` was not a header sipx knew.
- Mutation-tested five ways: notifying a terminated subscription, refreshing one back to life,
  accepting an unserved package, lengthening a requested expiry, and suppressing the implicit
  subscription on one side's say-so.

## Notes
- Gates the registration, dialog and presence packages — everything a busy-lamp field needs.
