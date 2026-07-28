---
id: T-19
title: Stop dropping incoming requests silently
pillar: Signalling
status: done
priority:
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport]
note: M7 · a full channel loses requests with no counter and no log
---

# Stop dropping incoming requests silently

## Goal
Make it impossible for the endpoint to lose an incoming request without anyone being able to tell.

## Acceptance
- [x] Delivery of `Incoming` no longer discards on a full channel without a trace. Today it is
      `let _ = self.incoming.try_send(…)` — the request is gone, nothing is logged, and no counter
      moves.
- [x] Whatever the policy becomes — await with backpressure, or shed deliberately — a shed request
      is **counted**, and the count is reachable from outside the endpoint the way
      `Handle::outstanding` already is.
- [x] Deliberate shedding, if that is the choice, sheds with a `503` rather than with silence,
      because a peer that is told to back off behaves better than one that is ignored.
- [x] Failing-first test: `a_request_dropped_for_backpressure_is_counted`.

## Progress
- Done. `Handle::shed()` returns a `ShedCounts` — requests, ACKs and unmatched requests, counted
  apart — and both delivery paths that used to drop silently now count and log.
- **The counter is an `Arc<AtomicU64>` shared with the driver, not a question asked of it.** The
  event loop is busy in exactly the situation this counts, so a metric readable only by asking the
  loop would be unavailable precisely when it is interesting. `Handle::outstanding` goes through
  the command channel because it reads state only the loop owns; this does not have to.
- **ACKs are counted apart, and the story's own note is why.** An ACK cannot be refused: SIP has no
  response to one, and an ACK for a 2xx is a transaction of its own (RFC 3261 §17.1.1.3) with
  nothing to answer. So a shed ACK gets no 503, nothing retransmits it after Timer H, and both ends
  are left in a dialog no timer reaps unless RFC 4028 session timers happen to be running. That is
  the one that leaks calls, so it is logged at `error` where the others are `warn`.
  - Worth noting what was already true: `refuse` looks up a server transaction, finds none for an
    ACK, and returns. So the old code did not send a malformed 503 to an ACK — it silently did
    nothing at all, which is the failure this story is about.
- The unmatched path had no transaction to refuse either, so counting and logging is the whole of
  what can be done there. The peer will retransmit an unmatched INVITE, which makes it the most
  survivable of the three — and it was still invisible.
- Mutation-tested: never incrementing the counter, dropping the `503` refusal, and incrementing
  unconditionally each fail the test that names the behaviour. The third matters as much as the
  others: without `an_endpoint_that_keeps_up_sheds_nothing`, a counter that always fired would
  have passed every other test here.

## Notes
- The severity depends on the message, which is why "the application was slow" is not an adequate
  answer. A dropped INVITE is a missed call and the peer retransmits. **A dropped ACK for a 2xx is
  a call that never ends**: the ACK is the only thing that concludes the transaction, nothing
  retransmits it after Timer H, and both ends sit in a dialog that no timer will reap until
  session timers (`S-11`) are in play — and those are optional.
- Raised alongside `T-18` by [sipx-clstr](https://github.com/codewandler/sipx-clstr)'s `PX-2`
  ([ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md)), but this one
  is a kernel defect on its own terms: silent, unmeasurable loss inside a stack whose whole
  premise is that its failure modes are testable.
