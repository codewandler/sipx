---
id: C-4
title: Serve many calls from one endpoint
pillar: Signalling
status: in-progress
priority: 4
design: docs/designs/app-sdk.md
epic: app-sdk
areas: [sipx-call]
note: app-sdk · after C-3 · size M
---

# Serve many calls from one endpoint

## Goal
One endpoint's stream of incoming requests, routed to any number of concurrent calls — with a
defined answer for every request that matches none of them — so a host can hold N calls without
writing its own demultiplexer.

## Acceptance
- [x] A dispatcher owns the endpoint's `Receiver<Incoming>` and routes each request to the call
      whose dialog (or transaction) it belongs to. A new INVITE that matches no call surfaces to
      the application as an incoming-call event; it is the application's decision to answer,
      ring or reject it.
- [x] Nothing is dropped silently: an in-dialog request matching no live call is answered
      **481 Call/Transaction Does Not Exist** (RFC 3261 §12.2.2), an unsupported method outside a
      dialog is refused with the status §8.2 prescribes, and both paths are tested. This story
      must not reintroduce at the call layer what `T-19` removes at the transport layer.
- [x] One stalled call must not stall its siblings: per-call delivery is channel-backed and
      bounded, and a full per-call queue has a defined, tested consequence for that call only
      (the vision's principle 3).
- [x] `serve()`'s single-call contract is either subsumed or explicitly documented as the
      one-call convenience over the dispatcher.
- [x] Failing-first test: `two_calls_served_concurrently_from_one_endpoint`.

## Progress
- Done. `crates/sipx-call/src/dispatch.rs` holds the `Dispatcher`, the routing table (`Calls`)
  and the counters; `docs/specs/call-dispatch.md` is the spec the tests are derived from, and
  `crates/sipx-call/tests/dispatch.rs` has thirteen of them.
- **The route key is `Call-ID` plus the *peer's* tag, never our own**, and that is the decision
  everything else hangs off. It is what lets a route be *reserved from the INVITE alone* — before
  the application has decided how to answer, and therefore before a local tag exists. The window
  that closes is real: the ACK to our own 2xx can arrive before `answer` has returned, and a route
  installed only afterwards would have nowhere to put it. The key stays unique per call in both
  directions (one INVITE makes at most one dialog here; a fork makes one dialog per *remote* tag),
  and `Dialog::matches` inside `Call::handle` still checks all three components — the key decides
  where a request goes, the dialog decides whether it belongs.
- **Every outcome is a response, a surfaced event or a counter**; there is no fourth. 481 for an
  orphaned in-dialog request (§12.2.2), 405 with `Allow` for an unsupported method (§8.2.1), 482
  for a merged INVITE (§8.2.2.2), 400 for a request naming no dialog at all (§8.1.1), 503 with a
  `Retry-After` when a call's own inbox is full, and *nothing but a counter and an error log* for
  an ACK — `T-19`'s reasoning one layer up, unchanged: SIP has no response to an ACK, so nothing
  retransmits it after Timer H and the dialog it would have completed is not reaped.
- **The 405's `Allow` is `sipx_sip::update::ALLOW` and nothing else**, which forced two decisions
  rather than one. Because that list names OPTIONS and CANCEL, an out-of-dialog request naming
  either is *surfaced* to the application instead of refused — a 405 whose own `Allow` says the
  method is supported tells a peer two different things. And because the same list is what a 405
  from `serve` carries, `Call::handle` now answers an in-dialog OPTIONS with 200 and the
  capability list (§11.2), through the dialog's own §12.2.2 guard rather than past it. A narrower
  second copy of the list would have been the drift `S-19` warns about.
- **`serve` no longer drops what `Call::handle` does not claim** (the story's own note): it
  answers 481 when the request is not this dialog's and 405 with `Allow` when it is but the method
  is not implemented. Its contract is documented as the one-call convenience over the dispatcher,
  and it is literally that — the dispatcher hands out a plain `Receiver<Incoming>`, so the same
  loop drives the endpoint's own receiver and a routed inbox with no second demultiplexer.
- **RFC 3311 §5.2, and what the dispatcher actually does for it.** `S-19` expected a concurrent
  dispatcher to make rules 1 and 2 reachable. It does not, and the finding is written down in
  `docs/specs/call-dispatch.md` §8 rather than left as an expectation: routing to N calls still
  serialises the requests *of one call*, because handling one needs `&mut Call`. What leaves a
  `Negotiation` non-idle across a `handle` boundary is an **abandoned exchange** — the shape
  `timeout(d, serve(..))` produces. The dispatcher is what makes that reachable in the way that
  matters: the peer's next request is off the wire and in the call's inbox *while* this side is
  mid-exchange, instead of unread behind a receiver nobody is polling. Both rules now have wire
  tests (491 for glare with no `Retry-After`, 500 with one in 0..=10 for an UPDATE in progress),
  and `abandon_after_one_poll` makes the abandonment deterministic rather than a race against a
  timeout.
- The registry gained `dispatch.rs` under RFC 3261 and RFC 3311, with the §8.2.1/§12.2.2/§8.2.2.2
  responses named; `docs/compliance.md` regenerated.
- **One window this does not close**, deliberately: `dial` returns only once the 2xx has arrived,
  so a BYE that overtakes the application's `Calls::register` draws a 481. Closing it needs the
  `Call-ID` known before the INVITE is sent, which is a change to `dial`'s surface this story does
  not sanction. Documented on `Calls::register`.

## Notes
- Today `serve()` drives exactly one call and drops whatever `Call::handle` does not claim
  (`crates/sipx-call/src/call.rs`, documented at the function). Every multi-call consumer must
  currently hand-roll this loop, and each hand-rolled copy is a fresh chance to drop an ACK.
- Needed by the host (`crates/sipx-app`, story `A-2`): a host serving webhook-driven calls
  is a multi-call consumer by definition.
- Depends on `C-3` for the incoming-call and per-call event delivery shape.
