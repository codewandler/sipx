---
id: C-7
title: Couple two dialogs without terminating media
pillar: Signalling
status: in-progress
priority: 17
design: docs/designs/edge.md
epic: edge
areas: [sipx-call, sipx-sdp]
note: RFC 7092 §3.1.3 · transparent SDP mapping · split from C-1
---

# Couple two dialogs without terminating media

## Goal
Provide the SDP-modifying signalling-only coupling role described by RFC 7092 §3.1.3: one owner
drives both dialogs and understands SDP, while sipx neither binds nor advertises local media
endpoints and remains entirely off the media path.

## Acceptance
- [x] A coupling can be created without constructing either leg's `MediaSession`, binding RTP or
      advertising a sipx media address; omitting `bridge_media` from two media-terminating calls is
      not accepted as equivalent.
- [ ] Offers and answers on every carrier supported by `C-1` are mapped transparently between the
      endpoint descriptions while preserving the independent per-dialog origin/version, security
      and direction invariants recorded in a normative spec.
- [x] Malformed or unmappable SDP is refused on its source leg before any peer-leg signalling is
      sent, with typed errors and no partially changed dialog.
- [x] BYE, CANCEL, glare and final-failure behaviour remains the same coupling policy as `C-1`; the
      off-media role does not grow a second lifecycle state machine.
- [x] A live causal test proves media addresses remain endpoint-owned and packets travel directly
      between endpoints while UPDATE or re-INVITE negotiation is relayed by sipx.
- [x] The RFC 7092 registry note upgrades only after that live proof demonstrates §3.1.3 rather
      than merely a coupling with no forwarding bridge.

## Progress
- Split from `C-1` when its final review established that every current `Call` still terminates a
  local media session even when no `Bridge` is attached.
- Added `sipx_sdp::relay::DescriptionRelay`, the sans-I/O mapping: it replaces the `o=` line with
  the receiving dialog's own origin, advances that dialog's version only when the rest of the
  description changed (RFC 3264 §8), and leaves every other byte alone. The rewrite is textual
  because re-serializing a parsed description normalizes line order, multicast TTLs and `m=` port
  counts — liberties an off-media element may not take with a description it is only carrying.
  Typed refusals: `Malformed`, `NoMedia`, `NoConnection`, surfaced as `sipx_call::Error::Relay`.
- Added `sipx_call::OffMediaCoupling`, which owns two `Dialog`s rather than two `Call`s. That is
  the whole difference: a `Call` binds an RTP socket before it can offer anything, so no flag on
  the media-terminating coupling could ever be truthful about §3.1.3. It reuses `CouplingState`
  unchanged — glare 491 decided before anything is forwarded, BYE mapped onto the peer, a target
  4xx/5xx returned as the source INVITE's own final response, CANCEL withdrawing the owned target
  invitation — and adds no second lifecycle machine.
- Failing-first test `media_addresses_stay_endpoint_owned_across_a_relayed_reinvite`: a raw source
  endpoint and a real sipx target exchange descriptions through the coupling, and the target's RTP
  arrives on a socket the test bound — then, after a relayed re-INVITE and a relayed UPDATE, at the
  ports those descriptions named. `a_media_terminating_coupling_replaces_the_endpoint_address` is
  its negative control: `EarlyCoupling::dial` with no bridge offers the target sipx's own port.
- Deliberately not covered, and refused rather than half-done: the reliable-provisional and PRACK
  carriers, foreclosed by not offering `100rel` (RFC 3262 §3), and offerless INVITEs in either
  direction, refused `488`. Both would need this role to *author* a description, which means
  describing a media endpoint it does not have. The second acceptance row stays open for that
  reason; the delayed-offer shape needs its own story.
- The RFC 7092 row stays `partial`, with the note rewritten: what is missing is no longer §3.1.3
  but those early carriers and the taxonomy roles sipx deliberately does not hold (§3.1.1, §3.2.1,
  §3.2.2).
- `coupling.rs` became `coupling/mod.rs`. Not cosmetic: `check-audio-claims.py` resolves a child
  module against its parent's directory, so the `coupling.rs` plus `coupling/` layout is a gate
  failure, and the nested layout is what `X-67` established for `call/`.

## Notes
- This is a signalling role, not a proxy, routing product or dial plan. Target selection remains
  outside the coupling primitive.
- RFC 7092 is an informational taxonomy. The normative protocol mechanics remain the cited offer/
  answer and dialog RFCs in the coupling spec.
