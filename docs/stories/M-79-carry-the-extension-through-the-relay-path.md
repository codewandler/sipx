---
id: M-79
title: Carry the header extension through the relay path
pillar: Media
status: ready
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

- [ ] `Encoded` carries the header extension alongside the payload type and bytes, or the relay
      path is restructured so the packet's extension reaches the far side intact.
- [ ] A failing-first test relays a packet carrying an extension through a bridge and asserts the
      forwarded bytes still hold it.
- [ ] The conference path is covered too, since it fans one source to several destinations and the
      extension must reach each.
- [ ] Dropping an extension, where that is correct for a given relay role, is a stated decision
      rather than an omission.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `M-75` and verified in the tree. That story made `Packet` carry its
  extension and round-trip byte-identically, which is the packet layer's half. The relay half is a
  different type: `crates/sipx-media/src/session.rs`'s `Encoded` holds exactly `payload_type` and
  `payload`, and `bridge.rs`'s `relay` takes an `Encoded`, so the extension is dropped at that
  conversion no matter what the packet preserved.

## Notes

- `M-75` deliberately did not widen `Encoded`: adding a field to it changes the shape every relay
  consumer sees, which is a decision about the relay contract rather than about packet parsing.
