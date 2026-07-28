---
id: C-4
title: Serve many calls from one endpoint
pillar: Signalling
status: backlog
priority:
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
- [ ] A dispatcher owns the endpoint's `Receiver<Incoming>` and routes each request to the call
      whose dialog (or transaction) it belongs to. A new INVITE that matches no call surfaces to
      the application as an incoming-call event; it is the application's decision to answer,
      ring or reject it.
- [ ] Nothing is dropped silently: an in-dialog request matching no live call is answered
      **481 Call/Transaction Does Not Exist** (RFC 3261 §12.2.2), an unsupported method outside a
      dialog is refused with the status §8.2 prescribes, and both paths are tested. This story
      must not reintroduce at the call layer what `T-19` removes at the transport layer.
- [ ] One stalled call must not stall its siblings: per-call delivery is channel-backed and
      bounded, and a full per-call queue has a defined, tested consequence for that call only
      (the vision's principle 3).
- [ ] `serve()`'s single-call contract is either subsumed or explicitly documented as the
      one-call convenience over the dispatcher.
- [ ] Failing-first test: `two_calls_served_concurrently_from_one_endpoint`.

## Progress
- Not started.

## Notes
- Today `serve()` drives exactly one call and drops whatever `Call::handle` does not claim
  (`crates/sipx-call/src/call.rs`, documented at the function). Every multi-call consumer must
  currently hand-roll this loop, and each hand-rolled copy is a fresh chance to drop an ACK.
- Needed by the host (`crates/sipx-app`, story `A-2`): a host serving webhook-driven calls
  is a multi-call consumer by definition.
- Depends on `C-3` for the incoming-call and per-call event delivery shape.
