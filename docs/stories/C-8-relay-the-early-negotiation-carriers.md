---
id: C-8
title: Relay the early negotiation carriers
pillar: Call
status: ready
priority: 35
design: docs/designs/edge.md
epic: edge
areas: [sipx-call, sipx-sdp]
predicate:
announcement:
note: C-7 refuses reliable provisionals, PRACK and offerless INVITE rather than half-relaying them · relaying those means authoring a description this role has no media for
---

# Relay the early negotiation carriers

## Goal

Extend the off-media coupling role to the negotiation carriers `C-7` deliberately refused, without
the coupling ever authoring a media description of its own.

## Acceptance

- [ ] A reliable provisional response carrying a description is relayed across both dialogs, with
      `100rel` no longer stripped from the target INVITE, and PRACK correlated on both legs.
- [ ] An offerless INVITE is relayed as a delayed offer rather than refused with 488, with the
      answer mapped back on the source leg.
- [ ] The `C-7` invariants hold unchanged on every new carrier: the coupling holds no
      `MediaSession`, binds no RTP, advertises no sipx address, and refuses an unmappable
      description on its source leg before the peer leg is told.
- [ ] A live causal test proves media flows endpoint-to-endpoint across each newly supported
      carrier, as `C-7`'s does for `InitialInvite`, `Update` and `Reinvite`.
- [ ] `docs/rfc/registry.toml`'s RFC 7092 evidence is extended in the same commit, and the status is
      raised only for what is actually proven — `C-7` deliberately left it `partial`.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `C-7`, which named this gap and could not file it because the board is
  fenced from implementors. `C-7` refuses rather than half-delivers: the target INVITE omits
  `100rel`, so RFC 3262 §3 forbids the peer sending a reliable provisional at all, and an offerless
  INVITE gets 488. Both carriers would require this role to author a description — that is, to
  describe a media endpoint it does not have — which is the one thing the role exists not to do.

## Notes

- `Invitation::answer_signalling` cannot carry a body today; a role reusing `SignallingCall` for
  this would need `prepare` to take one.
- `signalling::response_matches_dialog` hard-codes CSeq method `Bye`, so it is not reusable for
  UPDATE or re-INVITE responses. `C-7` built its own rather than widening it; this story may want
  to widen it properly.
