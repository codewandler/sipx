# Design: event reachability

**Status:** accepted · **Pillar:** Signalling · **Epic:** `event-reachability` · **Stories:** S-35,
S-37, S-38, S-39

## Why

sipx implements the notifier half of RFC 6665 in `crates/sipx-ua/src/subscribe.rs`: a subscription
store keyed on dialog and package, `Subscription-State` handling, 489 for an unserved package, and
three event packages — `dialog` (RFC 4235), `reg` (RFC 3680) and `presence` (RFC 3856) — plus PIDF
and PUBLISH with SIP-ETag soft state (RFC 3903).

None of it is reachable from a socket. Nothing in the workspace receives a SUBSCRIBE off the wire
and routes it into that store; the dispatcher answers orphaned in-dialog traffic with 481/405/482
and OPTIONS with 200, and SUBSCRIBE is not among the methods it serves. The registry rows and
`docs/maturity.md` both say so plainly, and the RFC 6665 row is `partial` for exactly this reason.

This is the sharpest instance of the pattern a 2026-08-04 capability review identified as sipx's
real feature gap: the code is not missing, the caller is. `X-37` already decided the doctrine —
reachability is resolved through callers, not by matching evidence paths — and a subsystem with no
caller is indistinguishable, from a user's seat, from one that was never written. It is worse than
absent, because the registry and the crate docs describe behaviour a user cannot invoke.

`S-24` (learn who is registered, via RFC 3680) sits on top of this and cannot be completed without
it: it consumes the generic subscriber rather than hiding a second event client inside discovery.
The same reachability defect exists for RFC 3903: publication state and entity tags work as library
logic, but no endpoint can receive or originate PUBLISH.

## Approach

- Serve SUBSCRIBE in the dispatcher alongside the methods it already answers, routing an accepted
  subscription into the existing store rather than introducing a second one. The store, the state
  machine and the package registry stay where they are; this story supplies the socket path and
  nothing else.
- Refusals stay explicit and typed: an unserved package is 489 as today, a subscription to a dialog
  that does not exist is 481, and an expiry outside the served range is negotiated down per
  RFC 6665 §4.2.1 rather than silently accepted.
- The notifier must send the initial NOTIFY that RFC 6665 §4.1.2 requires immediately on accepting
  a subscription — the property most worth a failing-first test, because a subscription that is
  accepted and then silent looks healthy on the wire and is not.
- Subscriptions are bounded like every other peer-driven resource in the workspace: a cap on
  concurrent subscriptions, and per-package state that cannot grow without one, matching the
  bounded-by-construction rule the transport layer already holds (`docs/designs/bounded-transports.md`).
- `S-37` specifies the reusable event-client contract before code. `S-38` then issues SUBSCRIBE,
  tracks `Subscription-State`, authenticates, refreshes and terminates it without giving any event
  package ownership of transport or timers. `S-24` is one consumer and remains responsible only for
  translating the `reg` package into peers.
- The contract is [`docs/specs/event-client.md`](../specs/event-client.md): NOTIFY establishes the
  subscriber route set, initial NOTIFY may beat the SUBSCRIBE response, one `Start` accepts one
  dialog rather than silently multiplying work through forks, and package parsing is a bounded
  injected consumer behind the generic lifecycle.
- `S-39` carries the existing RFC 3903 compositor and entity-tag lifecycle through live inbound and
  outbound PUBLISH paths. It reuses the store from `S-18`; it does not create a second presence
  service or durable publication database.

## Alternatives considered

- **Build subscriber and notifier socket paths together.** Rejected: they are separable, the
  notifier half is the one with an implementation already waiting, and one story that lands both
  is one story that lands neither for longer. The generic client follows its own specification.
- **Expose the subscription store as a public API and let applications route SUBSCRIBE themselves.**
  Rejected for the reason `docs/designs/edge.md` gives for `CouplingState`: making the protocol
  state machine the application's responsibility lets an application configure an invalid one. The
  dispatcher owns method routing today and should keep owning it.
- **Serve only the `dialog` package first.** Rejected as a false economy — the store is already
  keyed by package and the refusal path for an unserved one already exists, so restricting the set
  adds a decision without removing work.

## Risks and open questions

- An accepted subscription is a peer-driven timer and a peer-driven send: it is the first path in
  `sipx-ua` where a remote party causes sipx to originate traffic on a schedule. The bound and the
  shutdown path need the same treatment as the transport layer's, and the story must show the
  timers stop when the subscription is terminated, not merely that state is removed.
- NOTIFY establishes the dialog's route set and remote target. Subsequent in-dialog SUBSCRIBE uses
  that route/target through the ordinary transport connection pool in `docs/specs/sip-transport.md`
  §8; it does not resolve independently around the dialog.
- RFC 6665 permits package-specific fork handling. The generic client deliberately accepts the
  first dialog and refuses competing NOTIFY dialogs with 481, keeping one refresh/timer budget per
  application request.
