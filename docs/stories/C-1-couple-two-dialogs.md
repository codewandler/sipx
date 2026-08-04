---
id: C-1
title: Drive two dialogs as one call
pillar: Signalling
status: in-progress
priority:
design: docs/designs/edge.md
epic: edge
areas: [sipx-call, sipx-media]
note: M9 · RFC 7092 · the B2BUA primitive; the product stays out of this repo
---

# Drive two dialogs as one call

## Goal
One policy object owning two dialogs: an offer arriving on either leg is relayed to the other and
the answer relayed back, a re-INVITE or a BYE on one leg has a defined consequence on the other, and
`M-11`'s bridge carries the audio between them. The primitive a B2BUA is built from — not the B2BUA.

## Acceptance
- [x] A `Coupling` (name is the story's to choose) owns two calls and nothing else does. No shared
      mutable session between the legs: a stalled leg must not stall its peer, which is the
      [vision](../vision.md)'s principle 3 and the reason `M-11` moves frames over channels.
- [ ] An offer is relayed wherever it can legally arrive — the initial INVITE, a reliable provisional
      (`C-2`), a PRACK, an UPDATE (`S-19`) or a re-INVITE — and the answer relayed back on the same
      axis. An offer/answer state model per leg, not one shared one.
- [x] Glare is resolved rather than propagated. If a re-INVITE is outstanding on leg B when one
      arrives on leg A, leg A gets **491** and its UAC's fresh randomised retry is accepted after
      the outstanding exchange completes; the coupling does not forward a request the far end will
      refuse or replay a completed server transaction.
- [x] Terminating either leg terminates the other, with a defined mapping from the reason: a 4xx/5xx
      on the outbound leg becomes a final response on the inbound one; a BYE on either becomes a BYE
      on the other.
- [x] A CANCEL on the inbound leg cancels the outbound leg if it has not been answered, and does
      nothing that leaves a dialog behind if it has.
- [ ] A signalling-only coupling is possible: the same object with no media bridge attached, for the
      RFC 7092 §3.1.3 role — "understands SDP syntax but remains off the media path". Whether that
      is a mode or a consequence of not calling `bridge` is the design's choice, and it is recorded.
- [x] `docs/designs/edge.md` is updated with what was decided, including the two open questions it
      lists — how much of the policy is data versus a trait, and whether the signalling-only role is
      a separate mode.
- [x] The RFC registry entry for RFC 7092 is added, as an informational taxonomy sipx now positions
      itself in, with the roles it claims named.
- [x] Failing-first test: `a_bye_on_one_leg_ends_the_other`.

## Progress
- Implemented `sipx_call::CouplingState`, the sans-I/O policy table for the initial INVITE,
  reliable provisional, PRACK, UPDATE and re-INVITE axes, with a distinct offer/answer state per
  leg. It preserves outbound 4xx/5xx status and distinguishes CANCEL before and after confirmation.
- Implemented `EarlyCoupling`, which owns the inbound `Invitation`/`Ringing`, outbound `Dialing`,
  and both routed inboxes. It completes reliable-provisional PRACKs, relays offer-carrying UPDATEs
  before either INVITE has a final response, maps outbound final refusal onto the inbound INVITE,
  cancels a pending outbound leg, and BYEs an outbound leg whose 2xx crossed cancellation. It
  hands both confirmed calls and their inboxes over as `ConfirmedCoupling`.
- Added the owning initial constructor `EarlyCoupling::dial`: it consumes the source invitation
  before creating the target INVITE, maps the source audio direction onto fresh per-leg SDP, and
  retains cancellation responsibility while the target early dialog is established.
- Added the call primitives needed for RFC 3262's delayed-offer path. `ring_offer_early` originates an offer in a reliable
  provisional, `dial_early_without_offer` negotiates it and returns the answer in PRACK, and
  `Ringing::on_prack` adopts that answer into the same early media session confirmation inherits.
  The two-leg coupling still needs to hold the target PRACK until the source leg's provisional
  offer returns its answer; the standalone call proof does not claim that relay is complete.
- Implemented `sipx_call::Coupling`, which solely owns two confirmed `Call`s, relays UPDATE and
  re-INVITE negotiation in either direction, accepts a BYE before ending the peer with BYE, and
  treats a closed routed inbox as terminal rather than orphaning the other dialog. `bridge_media`
  explicitly attaches and rebuilds `M-11`'s bounded-channel bridge after renegotiation. Without a
  bridge the calls still terminate media locally, so this is not yet the signalling-only role.
- Confirmed offer relay preflights the source `Call` without mutation before opening coupling state
  or sending on the peer leg. Dialog identity, ordering, SDP syntax, active audio, negotiated codec,
  and renegotiation keying are therefore source-local refusals. Live regressions prove the far leg
  receives no request for a wrong dialog, malformed SDP, or valid SDP with no common codec.
- Added the normative state/lifecycle tables and byte-independent vectors in
  `docs/specs/call-coupling.md`, resolved both design questions in `docs/designs/edge.md`, and added
  RFC 7092 to the compliance registry with the `uac` and `uas` roles.
- Added failing-first test `a_bye_on_one_leg_ends_the_other`: it proves the offer crosses on the
  re-INVITE axis before the initiating response completes, bridged audio crosses, and a BYE on one
  leg is answered before the other leg receives its BYE. Executable early tests prove PRACK plus an
  offer-carrying UPDATE before confirmation, CANCEL propagation, and identical outbound/inbound
  final status. `the_owning_coupling_relays_the_initial_invite_offer` proves the owner creates and
  remains responsible for the target initial leg; `a_reliable_provisional_offer_is_answered_in_prack`
  proves both RFC 3262 carrier primitives and retained early media, but not yet their two-leg relay.
  Unit vectors cover the complete
  legal-axis policy table. The glare vector proves a live 491 and the request UAC's fresh retry after
  settlement; retaining and replaying the old `Incoming` would reuse a completed transaction.
- A coupling without an attached `Bridge` creates no forwarding tasks, but both owned `Call`s still
  advertise and terminate media locally. That is not RFC 7092 section 3.1.3's off-media-path role;
  a transparent SDP mapping that does not bind sipx media remains open, so the signalling-only
  acceptance item stays unchecked.

## Notes
- Scope discipline is the whole point of this story. It is the primitive, not the product: no
  listener configuration, no routes, no location service, no registrar, no recording. The
  [edge design](../designs/edge.md) records why — those are a product built with sipx, and the
  proxy, registrar and cluster roles are being built downstream on this kernel.
- RFC 7092 is informational and is used here for vocabulary, not for requirements: §3.1.1
  Proxy-B2BUA, §3.1.2 signalling-only, §3.1.3 SDP-modifying signalling-only, §3.2.1 media relay,
  §3.2.2 media aware, §3.2.3 media termination. sipx should be able to hold §3.1.3 and §3.2.3.
- This is not a proxy. A B2BUA is two user agents, which is precisely why it belongs in a user-agent
  stack while forking, Record-Route insertion and loop detection do not — see the
  [RFC roadmap](../rfc-roadmap.md)'s "deliberately not on this list".
- The hard cases are the early ones, which is why `S-19` and `C-2` come first. A coupling that only
  works after both legs are confirmed is a demo.
