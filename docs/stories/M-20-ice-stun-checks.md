---
id: M-20
title: Encode and answer a STUN connectivity check
pillar: Media
status: ready
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
- [ ] RFC 5769 §2.1's sample request is produced **byte-for-byte** from its stated username, password,
      `PRIORITY` and `ICE-CONTROLLED` tiebreaker. The encoder, not the decoder: the tag was computed
      by the IETF, so this is the direction that cannot be self-confirming.
- [ ] `MESSAGE-INTEGRITY` (RFC 5389 §15.4) over the length-adjusted message, then `FINGERPRINT`
      (§15.5) as CRC-32 XOR `0x5354554e` computed last, in that order; the received tag is compared
      constant-time (spec §11.2).
- [ ] Username direction: `<peer-ufrag>:<our-ufrag>` outbound keyed with the **peer's** password,
      `<our-ufrag>:<peer-ufrag>` inbound keyed with ours. Reversed, the agent answers nothing and its
      own checks are all rejected, and it looks exactly like a network fault.
- [ ] `PRIORITY`, `USE-CANDIDATE` (zero-length flag), `ICE-CONTROLLED`/`ICE-CONTROLLING`,
      `ERROR-CODE` 487 and `XOR-MAPPED-ADDRESS` encode as well as decode (spec §11.1).
- [ ] A §11 keepalive is a Binding **Indication** with `FINGERPRINT`, no credential and nothing else.
- [ ] No panic, no raw index and no wrapping length arithmetic on any byte string: this parser eats
      unauthenticated datagrams from anyone who can reach the media port.
- [ ] `sipx_transport::stun` is reused where it fits and gains nothing — it declares in its own header
      that ICE needs a different module, not more attributes bolted on.
- [ ] **This story makes the crate-graph decision** (spec §15): `MESSAGE-INTEGRITY` is HMAC-SHA1, and
      neither `sipx-media` nor `sipx-sdp` lists `hmac`/`sha1` while `sipx-rtp` does. Either the
      codec's crate gains two dependency lines, or the codec goes where they already are.
- [ ] Failing-first test: `a_connectivity_check_encodes_to_the_rfc_5769_sample_request`.

## Progress
- Not started. Cut from `M-16`'s proposed split; the Acceptance above is that proposal verbatim.

## Notes
- The spec is [`docs/specs/ice.md`](../specs/ice.md), written by `M-16` before any code. Read the
  sections its Acceptance names rather than re-deriving them from the RFCs.
- `M-16` is the tracker for this epic and stays open until every child is done.
