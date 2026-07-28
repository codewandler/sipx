---
id: S-11
title: Implement session timers
pillar: Signalling
status: done
priority: 3
design:
epic: conformance
areas: [sipx-call, sipx-sip]
note: track: signalling · RFC 4028 · headers already parse
---

# Implement session timers

## Goal
Detect a call whose far end has vanished without a BYE, which sipx currently keeps up forever.

## Acceptance
- [x] `Session-Expires` and `Min-SE` are negotiated on the INVITE and its response, with the
      refresher chosen per RFC 4028 §7.
- [x] The refresher sends a re-INVITE or UPDATE before the interval expires.
- [x] A session that is not refreshed is terminated locally with a BYE, and the media stops.
- [x] `Min-SE` below the configured floor is refused with 422 carrying the floor, rather than
      accepted and quietly ignored.
- [x] Failing-first test: `a_call_whose_far_end_vanishes_is_torn_down`.

## Progress
- Done. `sipx-sip::session` holds the pure half — header types, the Table 2 refresher election,
  the 422 rule and the two deadlines — and `sipx-call` drives it.
- **Two deadlines, not one**, because the roles do different things (§7.2, §10). The refresher
  acts at half the interval, which leaves the other half to notice a failure and retry. The
  other side waits until `interval - min(32s, interval/3)` and then hangs up: early, because the
  RFC's concern is a NAT that has already dropped the pinhole at the expiry instant, and a BYE
  sent after that tears the call down on one side only.
- **The floor is enforced in three places**, deliberately rather than as belt-and-braces. A
  configured floor below 90s is raised (a local misconfiguration should not make us an
  amplifier); an incoming request below the floor draws a 422 carrying it; and an interval
  arriving in a *2xx* is floored too. The last is the one that is easy to miss: §11.2's rogue
  UAS attack is a small `Session-Expires` in the response, and a defence the attacker supplies
  is not a defence.
- The refresh is a re-INVITE. UPDATE (RFC 3311) would be lighter; a re-INVITE is correct today
  and is what §7.2 describes.
- `Call::session_deadline` returns an instant rather than a future on purpose. A future would
  borrow the call for as long as it was awaited, which is the same borrow `handle` needs in the
  other arm of the `select!` it is written for. `serve()` wraps that loop so the RFC 4028 half
  is not something every caller has to remember.
- Mutation-tested: removing the teardown, the refresh-on-re-INVITE, or the `Min-SE` on the 422
  each fails exactly the test that names it.

## Notes
- The failure this fixes is invisible in a lab and permanent in production: a far end that
  loses power sends no BYE, and nothing else in sipx will ever notice.
