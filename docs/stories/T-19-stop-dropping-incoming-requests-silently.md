---
id: T-19
title: Stop dropping incoming requests silently
pillar: Signalling
status: ready
priority: 4
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport]
note: M7 · a full channel loses requests with no counter and no log
---

# Stop dropping incoming requests silently

## Goal
Make it impossible for the endpoint to lose an incoming request without anyone being able to tell.

## Acceptance
- [ ] Delivery of `Incoming` no longer discards on a full channel without a trace. Today it is
      `let _ = self.incoming.try_send(…)` — the request is gone, nothing is logged, and no counter
      moves.
- [ ] Whatever the policy becomes — await with backpressure, or shed deliberately — a shed request
      is **counted**, and the count is reachable from outside the endpoint the way
      `Handle::outstanding` already is.
- [ ] Deliberate shedding, if that is the choice, sheds with a `503` rather than with silence,
      because a peer that is told to back off behaves better than one that is ignored.
- [ ] Failing-first test: `a_request_dropped_for_backpressure_is_counted`.

## Progress
- Not started.

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
