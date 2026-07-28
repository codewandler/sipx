---
id: T-18
title: Surface unmatched responses to the application
pillar: Signalling
status: ready
priority: 10
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport]
note: track: reachability · the endpoint drops what a forwarding element is required to forward
---

# Surface unmatched responses to the application

## Goal
Let the application see a response that matched no client transaction, instead of the endpoint
logging it and dropping it.

## Acceptance
- [ ] A response reaching `Dispatch::Unmatched` is delivered to the application rather than
      discarded. Today `endpoint.rs` forwards only `Message::Request` from that arm
      (`if let Message::Request(request) = *message`), so responses fall on the floor.
- [ ] The delivery carries what a decision needs: the message, its source and its transport — the
      same shape `Incoming` already has for requests.
- [ ] A UA that does not care keeps not caring: ignoring the new events must remain the default
      that costs nothing.
- [ ] Failing-first test: `a_response_matching_no_transaction_reaches_the_application`.

## Progress
- Not started.

## Notes
- The current behaviour is right for a UA and wrong for a forwarding element. RFC 3261 §16.7 step
  1: a stateful proxy that finds no response context for a response **must forward it
  statelessly**. It cannot do that if it never sees the response.
- The shape is the open question — a second channel, a widened `Incoming` enum, or a callback.
  Whichever it is, a UA should not have to handle a case it has no answer for.
- Raised by [sipx-clstr](https://github.com/codewandler/sipx-clstr)'s `PX-2`, which decided to
  build its proxy driver on `sipx_transport::Handle` rather than on a socket loop of its own; this
  and `T-19` are the only two things that decision needs from the kernel
  ([ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md)).
