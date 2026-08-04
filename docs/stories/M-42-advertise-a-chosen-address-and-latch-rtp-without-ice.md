---
id: M-42
title: Advertise a chosen address and latch RTP without ICE
pillar: Media
status: ready
priority: 5
design: docs/designs/demand.md
epic: demand
areas: [sipx-transport, sipx-media, sipx-call, beta4]
predicate:
announcement: [3, 4]
note: the loudest unmet need in the surveyed ecosystem · most requesters are not doing ICE at all
---

# Advertise a chosen address and latch RTP without ICE

## Goal

Let an application advertise an address it chooses — in `Contact`, `Via` and the SDP `c=` line —
independently of the address sipx binds to, and keep media flowing when the peer's SDP advertises an
address it cannot actually receive on, without requiring ICE at either end.

## Acceptance

- [ ] **Establish what already holds first.** Progress records, with tests, whether sipx today can
      advertise a non-bind address per message and whether it latches RTP to the observed source.
      Closing this story as "already supported, now pinned by tests and documented" is a valid
      outcome and must not be padded with invented work.
- [ ] The advertised address is settable independently of the bind address and applies consistently
      to `Contact`, `Via` `sent-by` and the SDP connection line, proven by a test asserting all three.
- [ ] Symmetric RTP (latch to the source of received media, RFC 4961) works when the peer's SDP
      advertises an unreachable address, proven by a test where the advertised address is a
      black hole and audio still flows.
- [ ] `rport` and `received` handling (RFC 3581) is asserted for the registration and in-dialog
      paths, not only where it is already covered.
- [ ] Interaction with ICE is explicit: when ICE is enabled the ICE result wins, and the refusal or
      precedence is stated in the API documentation rather than left to discovery.
- [ ] An outbound-proxy `Route` can be configured so requests traverse a chosen next hop regardless
      of the request URI (RFC 3261 §8.1.2).
- [ ] The capability is reachable from the CLI, per vision principle 6, and documented in the
      library guides.
- [ ] `./scripts/gate.py` green.

## Progress
- (not started)

## Notes
- Highest-evidence gap in the 2026-08-04 demand survey — roughly twelve distinct requests with the
  deepest discussion threads in the corpus, and the requesters are overwhelmingly **not** doing ICE.
  sipx's ICE support solves a superset for peers that also do ICE, which is not this population.
- The failure mode users describe is one-way audio: the peer offers an internal address, media never
  arrives, and nothing in the signalling looks wrong.
- Keep this separate from `M-24` (relayed candidates). This story is the non-ICE path.
