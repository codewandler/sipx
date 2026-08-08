---
id: M-82
title: Say what a forwarded extension means to a peer that negotiated none
pillar: Media
status: done
priority: 20
design:
epic: media-interoperability
areas: [sipx-media, sipx-sdp]
predicate:
announcement:
note: sipx negotiates no extmap and now forwards element identifiers verbatim across a bridge
---

# Say what a forwarded extension means to a peer that negotiated none

## Goal

Resolve what a bridged RTP header extension means when neither leg negotiated `a=extmap`, either by
negotiating it or by recording why forwarding to a non-negotiating peer is safe.

## Why

`M-79` made a bridge forward the header extension it received. sipx negotiates no `extmap`, so the
bridge forwards element identifiers verbatim and cannot detect a peer that numbers them differently
— the identifiers are session-scoped by RFC 8285, so the same byte can mean two different things on
the two legs. The bridge module doc names this as the thing to revisit when `extmap` lands.

RFC 3550 §5.3.1 makes an unrecognised extension ignorable by a compliant receiver, which is the
argument for it being harmless today. That argument should be written down and checked rather than
inferred, particularly since browser offers carrying `a=extmap:` already appear in
`docs/specs/webrtc-audio.md`.

## Acceptance

- [x] Either `a=extmap` is negotiated and the bridge maps identifiers between the two legs, or the
      decision not to is recorded in the media spec with the RFC 3550 §5.3.1 argument stated and its
      limits named.
- [x] A test covers the two legs disagreeing about an identifier, and asserts whatever the decision
      promises.
- [x] The browser profile's position is stated: an offer carrying `a=extmap:` is answered
      consistently with the decision, rather than by silence.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from the independent review of `M-79`'s diff.
- 2026-08-08: **decided not to negotiate `extmap` and not to translate.** sipx negotiates no
  `a=extmap` in any profile, maps no element identifier between the legs of a bridge, and forwards
  the extension byte for byte; an offer carrying `a=extmap:` is answered by omitting it, which RFC
  8285 §7 makes the answerer's way of declining. Recorded in `docs/specs/media-runtime.md` §5 —
  argument in §5.2, limits in §5.3, the reopening trigger and the rejected alternative in §5.4 —
  and in `docs/specs/webrtc-audio.md` §4.1, §4.5, §10 and §11 for the browser profile.

  The argument, short form: sipx originates no identifier anywhere, so under RFC 8285 §7 it has
  none it may send and a relayed element is the only element on the outgoing packet — no receiver
  is ever given two meanings for one octet. A far end that negotiated none reads nothing out of it
  (RFC 8285 §5 makes the out-of-band mapping the definitive indication an extension is present),
  and one that does not implement RFC 8285 skips it whole, which is RFC 3550 §5.3.1's stated design
  goal and works because `M-85` makes the length word trustworthy. Decisive point: RFC 3550
  §5.3.1's profile field is scoped by the *profile*, not by a session, so for that kind of
  extension verbatim forwarding is the correct translation — **no single rule other than verbatim
  is right for both kinds**.

  Limits named in §5.3: it is about meaning and not disclosure (an element does cross two dialogs);
  it assumes a conformant receiver; two legs that coincidentally use the same identifier for
  different extensions are unobservable from the middle. §5.4's trigger: the day sipx originates an
  `a=extmap` on either leg, `Bridge`'s relay must learn to map identifiers **in the same change**.

- 2026-08-08: **all three new tests pin the decision; none is failing-first**, and that is stated in
  each test's own doc comment. There was nothing to fix — `M-79` already forwarded verbatim and both
  answer generators have always authored answers from local facts rather than echoing the offer. To
  show the assertions have teeth, each was run against a deliberate mutation (a `relay` that
  renumbers the first element to identifier 2; a `build` that emits `a=extmap:1 …`) and both failed
  with the intended message; the mutations were reverted and are not in the diff.

  - `crates/sipx-media/tests/relay_extension.rs::neither_leg_is_renumbered_when_the_two_disagree_about_an_identifier`
    — vector E1, both directions at once.
  - `crates/sipx-sdp/tests/browser_audio_profile.rs::an_offer_carrying_extmap_is_answered_by_omitting_it_rather_than_by_refusing_it`
    — vector E2.
  - `crates/sipx-sdp/src/answer.rs::an_offered_header_extension_mapping_is_neither_echoed_nor_originated`
    — the generic SIP answer path, which is the one a bridged leg actually uses.

- 2026-08-08: **CHANGELOG sentence owed** (not written here — the coordinator owns that file):
  "Settled what a bridged RTP header extension means to a peer that negotiated none: sipx
  negotiates no `a=extmap`, translates no element identifier between the legs of a bridge, and
  answers an offered `a=extmap` by omitting it. `docs/specs/media-runtime.md` §5 carries the RFC
  3550 §5.3.1 argument, its limits, and what would reopen it."

  No RFC registry change: RFC 8285 stays untracked, because adding a row would force a regeneration
  of `docs/compliance.md`, which is a coordinator-owned file. Adding an `[[rfc]]` row for 8285 with
  `status = "none"` is a reasonable follow-up for whoever next touches the registry.

- 2026-08-08: closed at the `1.0.0-rc.12` boundary, against the wave gate run on this tree.
