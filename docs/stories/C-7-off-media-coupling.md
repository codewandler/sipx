---
id: C-7
title: Couple two dialogs without terminating media
pillar: Signalling
status: backlog
priority:
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
- [ ] A coupling can be created without constructing either leg's `MediaSession`, binding RTP or
      advertising a sipx media address; omitting `bridge_media` from two media-terminating calls is
      not accepted as equivalent.
- [ ] Offers and answers on every carrier supported by `C-1` are mapped transparently between the
      endpoint descriptions while preserving the independent per-dialog origin/version, security
      and direction invariants recorded in a normative spec.
- [ ] Malformed or unmappable SDP is refused on its source leg before any peer-leg signalling is
      sent, with typed errors and no partially changed dialog.
- [ ] BYE, CANCEL, glare and final-failure behaviour remains the same coupling policy as `C-1`; the
      off-media role does not grow a second lifecycle state machine.
- [ ] A live causal test proves media addresses remain endpoint-owned and packets travel directly
      between endpoints while UPDATE or re-INVITE negotiation is relayed by sipx.
- [ ] The RFC 7092 registry note upgrades only after that live proof demonstrates §3.1.3 rather
      than merely a coupling with no forwarding bridge.

## Progress
- Split from `C-1` when its final review established that every current `Call` still terminates a
  local media session even when no `Bridge` is attached.

## Notes
- This is a signalling role, not a proxy, routing product or dial plan. Target selection remains
  outside the coupling primitive.
- RFC 7092 is an informational taxonomy. The normative protocol mechanics remain the cited offer/
  answer and dialog RFCs in the coupling spec.
