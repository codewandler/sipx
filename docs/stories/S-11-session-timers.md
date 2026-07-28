---
id: S-11
title: Implement session timers
pillar: Signalling
status: ready
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
- [ ] `Session-Expires` and `Min-SE` are negotiated on the INVITE and its response, with the
      refresher chosen per RFC 4028 §7.
- [ ] The refresher sends a re-INVITE or UPDATE before the interval expires.
- [ ] A session that is not refreshed is terminated locally with a BYE, and the media stops.
- [ ] `Min-SE` below the configured floor is refused with 422 carrying the floor, rather than
      accepted and quietly ignored.
- [ ] Failing-first test: `a_call_whose_far_end_vanishes_is_torn_down`.

## Progress
- Not started. Both headers already parse (`compliance.md` lists RFC 4028 as syntax-only), so
  this is behaviour and negotiation only.

## Notes
- The failure this fixes is invisible in a lab and permanent in production: a far end that
  loses power sends no BYE, and nothing else in sipx will ever notice.
