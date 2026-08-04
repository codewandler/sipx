---
id: M-42
title: Advertise a chosen address and latch RTP without ICE
pillar: Media
status: done
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

- [x] **Establish what already holds first.** Progress records, with tests, whether sipx today can
      advertise a non-bind address per message and whether it latches RTP to the observed source.
      Closing this story as "already supported, now pinned by tests and documented" is a valid
      outcome and must not be padded with invented work.
- [x] The advertised address is settable independently of the bind address and applies consistently
      to `Contact`, `Via` `sent-by` and the SDP connection line, proven by a test asserting all three.
- [x] Symmetric RTP (latch to the source of received media, RFC 4961) works when the peer's SDP
      advertises an unreachable address, proven by a test where the advertised address is a
      black hole and audio still flows.
- [x] `rport` and `received` handling (RFC 3581) is asserted for the registration and in-dialog
      paths, not only where it is already covered.
- [x] Interaction with ICE is explicit: when ICE is enabled the ICE result wins, and the refusal or
      precedence is stated in the API documentation rather than left to discovery.
- [x] An outbound-proxy `Route` can be configured so requests traverse a chosen next hop regardless
      of the request URI (RFC 3261 §8.1.2).
- [x] The capability is reachable from the CLI, per vision principle 6, and documented in the
      library guides.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-04: implementation started by pinning the bind/advertise, symmetric-RTP and ICE
  precedence contracts in `docs/specs/deployment-addresses.md`. Reconnaissance found that
  signalling already separates `Config::bind` from Via `sent_by`, preloaded Route is already
  exposed by `DialOptions::with_service_route`, and the media session already has a symmetric-RTP
  path. The concrete missing deployment seam is media: the call layer currently binds its RTP
  socket to the same `media_address` it serializes into SDP, so an advertised NAT address which is
  not locally owned cannot be used. The CLI likewise has no explicit advertised-address input.
- 2026-08-04: the baseline was established before changing the seam. Existing passing evidence was
  `sipx-media --test ice::a_peer_that_offers_no_ice_keeps_symmetric_rtp` (a bound but unread SDP
  destination is bypassed after valid RTP arrives), `sipx-call --test service_route` (Route order
  and unchanged Request-URI), the transport-wide RFC 3581 mutation in `nat.rs`, and the call
  re-offer paths retaining `Call::media_address`. Signalling already separated `Config::bind`
  from `sent_by`; media did not separate its two roles.
- 2026-08-04: added the deployment-address seam. `DialOptions::with_media_bind_address` separates
  an outbound RTP bind from its existing advertised `media_address`; `MediaAddress` and new `*_at`
  inbound/early-media functions do the same without changing the legacy `IpAddr` function
  signatures. Adding the public `DialOptions::media_bind_address` field is a deliberate beta API
  break for external struct literals and exhaustive patterns; they must add the field or migrate to
  `DialOptions::new` and builders. Constructor-based calls retain bind-equals-advertise behavior.
  `Call` and early sessions retain both values, so re-INVITE/UPDATE answers advertise the original
  public address while a replacement socket binds locally. An unspecified advertised address now
  returns typed `Error::UnspecifiedMediaAddress` before an INVITE, answer or early response leaves;
  the test also asserts the peer queue is empty.
- 2026-08-04: `crates/sipx-call/tests/deployment_addresses.rs` supplies `198.51.100.44` as the
  unbindable advertised address and loopback as the media bind, then inspects one INVITE and finds
  the same host in Contact, Via sent-by and SDP `c=`. `crates/sipx-cli/tests/deployment_addresses.rs`
  drives the real `sipx dial --local ... --advertise ...` process through that vector, proving the
  flag reaches the library rather than merely appearing in help. Dial and answer JSON now report
  `media_advertised` and the running socket's `media_bound`.
- 2026-08-04: explicit coverage now names the surrounding contracts. The transport UDP integration
  asserts both observed `received` and `rport` for REGISTER. The call integration establishes a
  dialog through INVITE, 200 and ACK before asserting those fields on its real BYE. The ICE unit
  test admits valid RTP from a non-nominated source with ICE ownership enabled and proves the
  destination remains the nominated pair. The symmetric-RTP test offers a bound, unread sink and
  proves both that return audio reaches the observed source and that none reaches that offered
  destination. The Service-Route test now states the exact API contract: the caller supplies its
  resolved proxy `Target`, the call layer serializes the Route set, and Request-URI stays distinct.
- 2026-08-04: inbound and early-media coverage then made the seam explicit in every initial SDP
  role: a raw inbound INVITE receives a `198.51.100.44` answer while the returned session is bound
  to loopback, and one construction test checks both reliable-provisional shapes (local offer in
  183 and local answer in 183) against the same advertised/bind pair. The four pre-audit
  deployment-address integration tests plus that early-media unit test were green at this point.
- 2026-08-04: public example sources were updated and `sync-website.py --update` refreshed the
  generated answer/place guide regions. After the concurrent comparison regeneration landed,
  `comparison-report.py --check` and `sync-website.py --check` both passed. CLI reference generation
  and its six tests are green; formatting, diff hygiene and the fixed-sleep check are green. The
  final four-crate all-target clippy rerun got past the concurrent SDP finding and then stopped only
  because the shared target hit its disk quota; root owns the post-cleanup combined check.
- 2026-08-04: release audit made the public examples use the same advertised host for Via, Contact
  and SDP, replaced the synthetic standalone-BYE witness with the established-dialog test above,
  and changed the real CLI success scenario to parse and assert `media_advertised` and
  `media_bound` on both endpoints. A wildcard `sipx answer` now asks the routing table for its
  reachable local interface instead of copying the caller's source IP. The relocated verification
  found that binding the implicit CLI media path to the wildcard suppressed all ICE candidates; the
  no-override default now binds the route-selected local address, while an explicit `--advertise`
  still retains `--local` as the independent bind. The server-reflexive ICE CLI scenario pins that
  composition with the mux-capable initial offer.

## Notes
- Highest-evidence gap in the 2026-08-04 demand survey — roughly twelve distinct requests with the
  deepest discussion threads in the corpus, and the requesters are overwhelmingly **not** doing ICE.
  sipx's ICE support solves a superset for peers that also do ICE, which is not this population.
- The failure mode users describe is one-way audio: the peer offers an internal address, media never
  arrives, and nothing in the signalling looks wrong.
- Keep this separate from `M-24` (relayed candidates). This story is the non-ICE path.
