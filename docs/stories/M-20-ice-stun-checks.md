---
id: M-20
title: Encode and answer a STUN connectivity check
pillar: Media
status: in-progress
priority: 8
design: docs/designs/media.md
epic: ice
areas: [sipx-media]
note: ice · RFC 5389/5769 · carries the crate-graph decision; runs solo, it moves the lockfile
---

# Encode and answer a STUN connectivity check

## Goal
A STUN codec that can both send and answer an ICE connectivity check — the attributes, the
credentials and the two integrity values — so the agent has a transaction to run.

## Acceptance
- [x] RFC 5769 §2.1's sample request is produced **byte-for-byte** from its stated username, password,
      `PRIORITY` and `ICE-CONTROLLED` tiebreaker. The encoder, not the decoder: the tag was computed
      by the IETF, so this is the direction that cannot be self-confirming.
- [x] `MESSAGE-INTEGRITY` (RFC 5389 §15.4) over the length-adjusted message, then `FINGERPRINT`
      (§15.5) as CRC-32 XOR `0x5354554e` computed last, in that order; the received tag is compared
      constant-time (spec §11.2).
- [x] Username direction: `<peer-ufrag>:<our-ufrag>` outbound keyed with the **peer's** password,
      `<our-ufrag>:<peer-ufrag>` inbound keyed with ours. Reversed, the agent answers nothing and its
      own checks are all rejected, and it looks exactly like a network fault.
- [x] `PRIORITY`, `USE-CANDIDATE` (zero-length flag), `ICE-CONTROLLED`/`ICE-CONTROLLING`,
      `ERROR-CODE` 487 and `XOR-MAPPED-ADDRESS` encode as well as decode (spec §11.1).
- [x] A §11 keepalive is a Binding **Indication** with `FINGERPRINT`, no credential and nothing else.
- [x] No panic, no raw index and no wrapping length arithmetic on any byte string: this parser eats
      unauthenticated datagrams from anyone who can reach the media port.
- [x] `sipx_transport::stun` is reused where it fits and gains nothing — it declares in its own header
      that ICE needs a different module, not more attributes bolted on.
- [x] **This story makes the crate-graph decision** (spec §15): `MESSAGE-INTEGRITY` is HMAC-SHA1, and
      neither `sipx-media` nor `sipx-sdp` lists `hmac`/`sha1` while `sipx-rtp` does. Either the
      codec's crate gains two dependency lines, or the codec goes where they already are.
- [x] Failing-first test: `a_connectivity_check_encodes_to_the_rfc_5769_sample_request`.

## Progress
The codec is [`sipx_media::ice::stun`](../../crates/sipx-media/src/ice/stun.rs). 25 tests, gate
green. `M-16`'s open question in spec §15 is answered below and can be struck from it.

Review round: a doc reference to a `cfg(test)` anchor that rendered as a 404 on the published site,
the `Attribute::Unknown` hole below, `ERROR-CODE` folding out-of-range codes into range instead of
refusing them, spec §11.1's `USERNAME` row, and a `check-features.sh` guard for the
`sipx-transport` edge this story adds — that edge takes `default-features = false`, and nothing
about the build notices if the flag is dropped, so the assertion is on the resolved graph.

### The crate-graph decision: `sipx-media`, and it gains three dependency lines

**Decided: `sipx-media`.** It gains `hmac`, `sha1` and `subtle`, plus `sipx-transport` with default
features off. The reasoning, including what was rejected:

- **Rejected: `sipx-rtp`, where `hmac`/`sha1`/`subtle` already are.** It is the option that changes
  no manifest, and it is the wrong one. `sipx-rtp` is "RTP and RTCP packet handling" and a
  connectivity check is not a media packet; every downstream user who wants only to parse RTP would
  inherit a STUN codec in that crate's public API and its published docs. It would also put the
  codec a crate *below* its only caller — spec §15 puts the agent in `sipx-media` — so the codec and
  the state machine that drives it would sit either side of a crate boundary for a manifest-line
  reason. A dependency added to a crate is inherited by everyone downstream, and so is a module.
- **Accepted: `sipx-media`, three lines.** The decisive fact is that all three crates are *already*
  in `sipx-media`'s transitive graph through `sipx-rtp`, so naming them directly adds nothing to
  anybody's build — no new crate is fetched, compiled or locked. The cost is three lines of manifest;
  the benefit is that the codec lives where spec §15 already puts the agent and the driver.
- **Three, not the two the Acceptance names.** `subtle` is the third. Spec §11.2 names it
  explicitly for the constant-time tag comparison, and it is what `sipx_sdp::fingerprint` and
  `sipx_rtp::srtp` both use for the same job. `hmac::Mac::verify_slice` would have been constant-time
  too and saved the line, but it hides the property behind a call whose name does not say so, in the
  one place a reviewer most needs to see it.
- **`sipx-transport`, spelled out rather than inherited.** Acceptance wants
  `sipx_transport::stun` reused, and reuse needs the edge. Spec §15 already routes reflexive
  gathering through it ("The Binding client already exists; a second one would be a second thing to
  get wrong"), so the edge is sanctioned rather than invented here. It carries
  `default-features = false`, because what is being borrowed is RFC 5389's twenty-byte header and
  not a SIP transport — with defaults on, every user of `sipx-media` would inherit rustls, a
  WebSocket stack and a DNS client. Cargo refuses `default-features = false` on a workspace
  dependency that does not set it, and setting it in the root table would turn the transports off
  for the five crates that want them, so this one dependency is written out with its version. A
  release that bumps the workspace version and forgets that line fails in `cargo publish`, loudly.

**What was reused, and what was not.** `MAGIC_COOKIE`, `HEADER_LEN`, `TransactionId`,
`new_transaction_id` and `is_stun` come from `sipx_transport::stun` unchanged, and that module gained
nothing. Its `XOR-MAPPED-ADDRESS` reader did *not* fit: it is private, reachable only through
`parse_reply`, which reads Binding Responses and drops every attribute ICE needs — and ICE has to
write the attribute as well as read it. Exposing the helper would have been extending the module the
Acceptance says must not be extended.

### A response carries no `USERNAME` — and spec §11.1 was wrong, not the code

Spec §11.1's `USERNAME` row read "every check and its response". Both RFCs contradict it outright:
RFC 5389 §10.1.2, "The response MUST NOT contain the USERNAME attribute", and RFC 8445 §7.2.2,
"(note that the USERNAME attribute is not present in the response)". An encoder written from the
old row could not have reproduced RFC 5769 §2.2 at all. **§11.1 is corrected in this story**, in the
shape `M-19` used for §6.2: the row now reads "every check; **never a response**", with a dated
attribution paragraph under the table saying what it used to say and why it was wrong. `M-21` and
`M-22` build on §11, so the correction belongs here rather than in whichever story trips over it.

### `PRIORITY` is range-checked, and the interop cost is accepted knowingly

`Message::decode` rejects the **whole datagram** when `PRIORITY` is outside RFC 8839 §5.1's
`1..=2^31−1`, because `sipx_sdp::ice::Priority` will not hold the value. Taken as a decision, not
inherited by accident:

- Spec §6.2 is explicit that the range check on parse is what keeps §6.1.2.3's pair-priority
  arithmetic inside a `u64`, and RFC 8445 §5.1.2.1's formula cannot reach 2^31 for a conforming
  peer. Nothing legitimate is refused.
- The residual risk is real: a peer that treats `PRIORITY` as a plain `u32` and sets the high bit
  has **every** check dropped, and the failure has exactly the signature this story's own
  Acceptance warns about for the username direction — it looks like a blocked path and gets
  diagnosed as a network fault.
- Rejected alternative: accept the wide value and range-check where the arithmetic happens. That
  moves an overflow a peer chooses into a crate that cannot see where the number came from.

Failing closed at the parser is worth the interop risk; failing closed *silently* would not be, so
it is written into the module documentation as well as here. If a real peer is ever found doing
this, the fix is a counter and a warning at the drop site, not a widened bound.

### Two smaller things a later story should know

- **Attribute padding is `0x20`,** not zero. RFC 5389 §15 says the padding "may be any value", but
  `MESSAGE-INTEGRITY` is an HMAC over it, so the choice is visible in the bytes: both RFC 5769
  vectors pad with `0x20`, and an encoder padding with zeroes reproduces neither published tag.
- **`Attribute::Unknown` cannot name `MESSAGE-INTEGRITY` or `FINGERPRINT`.** The module claims the
  two integrity values are computed and never supplied; `Unknown` was the hole in that claim, since
  it re-encodes any type verbatim. Encoding one now returns `Error::ReservedAttribute`. Unreachable
  from the wire — `decode` matches both types before it builds an `Unknown` — but very reachable
  from `M-21`, which assembles messages by hand, and two `MESSAGE-INTEGRITY` attributes in one
  message authenticate as nothing.

## Notes
- The spec is [`docs/specs/ice.md`](../specs/ice.md), written by `M-16` before any code. Read the
  sections its Acceptance names rather than re-deriving them from the RFCs.
- `M-16` is the tracker for this epic and stays open until every child is done.
