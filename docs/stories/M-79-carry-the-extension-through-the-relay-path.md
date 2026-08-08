---
id: M-79
title: Carry the header extension through the relay path
pillar: Media
status: in-progress
priority: 5
design:
epic: media
areas: [sipx-media, sipx-rtp]
predicate:
announcement:
note: M-75 preserved the extension at the packet layer, but Encoded carries only a payload type and bytes, so bridge and conference still drop it
---

# Carry the header extension through the relay path

## Goal

Let a bridged or conferenced call forward the RTP header extension it received, rather than dropping
it at the boundary between the packet and the relay.

## Acceptance

- [x] `Encoded` carries the header extension alongside the payload type and bytes, or the relay
      path is restructured so the packet's extension reaches the far side intact.
- [x] A failing-first test relays a packet carrying an extension through a bridge and asserts the
      forwarded bytes still hold it.
- [x] The conference path is covered too, since it fans one source to several destinations and the
      extension must reach each.
- [x] Dropping an extension, where that is correct for a given relay role, is a stated decision
      rather than an omission.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `M-75` and verified in the tree. That story made `Packet` carry its
  extension and round-trip byte-identically, which is the packet layer's half. The relay half is a
  different type: `crates/sipx-media/src/session.rs`'s `Encoded` holds exactly `payload_type` and
  `payload`, and `bridge.rs`'s `relay` takes an `Encoded`, so the extension is dropped at that
  conversion no matter what the packet preserved.

- 2026-08-08: implemented. `Encoded` now carries `extension: Option<Bytes>` beside the payload
  type and bytes, `Frame::Encoded` carries it across the send queue, `deliver` fills it from the
  received packet and the send loop writes it back onto the outgoing packet. Everything else about
  a relayed packet is still this leg's own — sequence, timestamp, SSRC — because the two legs are
  separate RTP streams; the extension is not, because it describes the media rather than the stream
  carrying it.

  Red first, at `b66d230` with only the new test file present:

  ```
  $ cargo test -p sipx-media --all-features --test relay_extension
  running 3 tests
  test a_bridged_packet_reaches_the_far_side_with_its_extension ... FAILED
  test a_fan_out_carries_the_extension_to_every_destination ... FAILED
  test a_conference_mix_carries_no_contributors_extension ... ok

  ---- a_bridged_packet_reaches_the_far_side_with_its_extension stdout ----
  assertion `left == right` failed: the bridge delivered the payload without the header
  extension it arrived with, so the far end reads this media differently from the way its
  sender meant it
    left: None
   right: Some([190, 222, 0, 1, 16, 170, 0, 0])
  ```

  All three pass now. `crates/sipx-media/tests/relay_extension.rs` asserts on the far side's wire
  bytes rather than on session types, because what leaves the socket is what the far end acts on.

  **Where the extension does not travel, stated rather than omitted.** A conference is a mixer in
  the RFC 3550 §7.1 sense: the packet a participant receives is one this endpoint authored from the
  sum of the others, so it is not any contributor's packet and there is nothing on it for a
  contributor's RFC 8285 element to describe — with several contributors there is not even a rule
  that would pick whose to attach. The drop is structural, since mixing works on decoded samples;
  `conference.rs`'s module documentation records the decision and
  `a_conference_mix_carries_no_contributors_extension` pins it. The same paragraph names the three
  bridge cases that carry nothing (transcoding, an unnegotiated payload type, a muted leg), and
  `realtime.rs` records that its bridge terminates RTP rather than relaying it, so an extension has
  no packet to travel on.

  **The acceptance row about the conference was written against a premise the code does not have.**
  `Conference` does not fan encoded bytes to several destinations; it mixes samples and never
  touches `Encoded`. The fan-out property the row is about — one relayed value cloned to several
  destinations, each receiving the extension — is covered directly by
  `a_fan_out_carries_the_extension_to_every_destination`, which was red at the base.

  API note for the coordinator: `sipx_media::Encoded` gained a public field, so struct-literal
  construction breaks. `Encoded::new(payload_type, payload)` is the migration for a caller
  authoring its own payload, and is what the four in-repo literal sites now use.

  CHANGELOG sentence, if one is wanted: *A bridged call now forwards the RTP header extension it
  received rather than dropping it at the relay boundary; `sipx_media::Encoded` carries the
  extension, and `Encoded::new` builds a payload this endpoint authored.*

## Notes

- `M-75` deliberately did not widen `Encoded`: adding a field to it changes the shape every relay
  consumer sees, which is a decision about the relay contract rather than about packet parsing.
