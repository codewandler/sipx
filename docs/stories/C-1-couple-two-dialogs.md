---
id: C-1
title: Drive two dialogs as one call
pillar: Signalling
status: backlog
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
- [ ] A `Coupling` (name is the story's to choose) owns two calls and nothing else does. No shared
      mutable session between the legs: a stalled leg must not stall its peer, which is the
      [vision](../vision.md)'s principle 3 and the reason `M-11` moves frames over channels.
- [ ] An offer is relayed wherever it can legally arrive — the initial INVITE, a reliable provisional
      (`C-2`), a PRACK, an UPDATE (`S-19`) or a re-INVITE — and the answer relayed back on the same
      axis. An offer/answer state model per leg, not one shared one.
- [ ] Glare is resolved rather than propagated. If a re-INVITE is outstanding on leg B when one
      arrives on leg A, leg A gets **491** and the coupling retries after the outstanding exchange
      completes; it does not forward a request the far end will refuse.
- [ ] Terminating either leg terminates the other, with a defined mapping from the reason: a 4xx/5xx
      on the outbound leg becomes a final response on the inbound one; a BYE on either becomes a BYE
      on the other.
- [ ] A CANCEL on the inbound leg cancels the outbound leg if it has not been answered, and does
      nothing that leaves a dialog behind if it has.
- [ ] A signalling-only coupling is possible: the same object with no media bridge attached, for the
      RFC 7092 §3.1.3 role — "understands SDP syntax but remains off the media path". Whether that
      is a mode or a consequence of not calling `bridge` is the design's choice, and it is recorded.
- [ ] `docs/designs/edge.md` is updated with what was decided, including the two open questions it
      lists — how much of the policy is data versus a trait, and whether the signalling-only role is
      a separate mode.
- [ ] The RFC registry entry for RFC 7092 is added, as an informational taxonomy sipx now positions
      itself in, with the roles it claims named.
- [ ] Failing-first test: `a_bye_on_one_leg_ends_the_other`.

## Progress
- Not started. `M-11` bridges *media* between two calls (`sipx-media/src/bridge.rs`, with a raw relay
  path on the session). Nothing couples the two dialogs' signalling, so a bridge today survives its
  legs diverging only because nobody tells it they have.

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
