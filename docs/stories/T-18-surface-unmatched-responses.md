---
id: T-18
title: Surface unmatched responses to the application
pillar: Signalling
status: done
priority:
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport]
note: M7 · the endpoint drops what a forwarding element is required to forward
---

# Surface unmatched responses to the application

## Goal
Let the application see a response that matched no client transaction, instead of the endpoint
logging it and dropping it.

## Acceptance
- [x] A response reaching `Dispatch::Unmatched` is delivered to the application rather than
      discarded. Today `endpoint.rs` forwards only `Message::Request` from that arm
      (`if let Message::Request(request) = *message`), so responses fall on the floor.
- [x] The delivery carries what a decision needs: the message, its source and its transport — the
      same shape `Incoming` already has for requests.
- [x] A UA that does not care keeps not caring: ignoring the new events must remain the default
      that costs nothing.
- [x] Failing-first test: `a_response_matching_no_transaction_reaches_the_application`.

## Progress
- Done. `Handle::watch_unmatched(capacity)` installs a sink and returns a receiver of `Unmatched`
  — the response, its source and its transport, the same shape `Incoming` has for requests.
- **The open question in the notes is answered: an opt-in sink, not a widened `Incoming`.** The
  acceptance says a UA that does not care must keep not caring, and that rules out the other two
  shapes on its own. Widening `Incoming` into an enum makes every existing user agent handle a case
  it has no answer for; a second channel out of `bind` changes the signature for everyone. An
  endpoint nobody is watching allocates no channel and behaves exactly as before.
- Calling it twice **replaces** the sink rather than fanning out. Two watchers would each see some
  of the responses and neither would see all of them — a subtler failure than having no watcher,
  and one that would look like packet loss.
- `try_send`, and a full watcher channel is the watcher's problem: blocking the driver here would
  stop every timer in the endpoint while waiting for a consumer that is already behind. A drop
  counts against `ShedCounts::unmatched` (`T-19`), so this path is not a new silent one.
- Mutation-tested both ways round: dropping unmatched responses again fails the delivery test, and
  counting them as shed when nobody asked for them fails the test that says an unwatched endpoint
  is undisturbed. The second matters — without it, "deliver to nobody" and "drop deliberately"
  would be indistinguishable.

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
