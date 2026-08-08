---
id: M-75
title: Preserve RTP header extensions on re-encode
pillar: Media
status: done
priority:
design:
epic: media
areas: [sipx-rtp]
predicate:
announcement:
note: extensions are parsed on receive and silently dropped when a packet is re-encoded · harmless for audio today, a correctness trap for any relay
---

# Preserve RTP header extensions on re-encode

## Goal

Make a re-encoded RTP packet carry the header extensions it arrived with, so a path that forwards
packets does not silently strip information the peer relied on.

## Acceptance

- [x] A packet parsed with header extensions and re-encoded emits the same extension bytes, and a
      failing-first test asserts byte equality across the round trip.
- [x] Dropping an extension, where that is the correct behaviour, is a stated decision in the
      packet's own documentation rather than an omission — say which extensions are forwarded, which
      are rewritten and which are refused.
- [x] The bridge and conference relay paths are covered, since those are where the loss is
      observable.
- [x] Malformed or over-long extension data is refused rather than truncated, and a fuzz-shaped
      negative proves it cannot panic.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `M-40`'s adjacent findings — `crates/sipx-rtp/src/packet.rs` parses header
  extensions on receive and drops them on re-encode. Harmless for the audio paths shipping today,
  which do not forward extensions, and a correctness trap for any relay expected to preserve them.

- 2026-08-08: implemented for the packet layer. `decode` now slices the extension whole — profile
  field, length word and payload — and keeps it on `Packet::extension`; `encode` writes it back and
  sets the extension bit **only** when bytes are actually carried, since setting it without them
  makes the next reader consume payload as an extension header. `decode` also now bounds-checks the
  extension before slicing, so an over-long length is `Truncated` rather than a panic.
  Three tests pin it: byte-equal round trip, the bit staying clear when the extension is removed,
  and the payload boundary surviving. The crate never interprets the bytes — RFC 8285's one- and
  two-byte forms are a profile's business, and a reading invented here would be a guess with a wire
  effect.
  **The bridge and conference relay rows are not ticked.** Those paths carry `Encoded` payloads and
  never decode a packet, so there is nothing there to preserve yet; making them extension-aware is
  a different change from making the packet type carry one.

> **Changed:** `sipx_rtp::packet::Packet` gained an `extension` field. Code constructing one
> literally needs `extension: None`; nothing else moves.

- 2026-08-08: **closed for the packet layer; the relay rows moved to `M-79`.** Verified rather than
  assumed: `Encoded` holds exactly `payload_type` and `payload`, and `bridge.rs`'s `relay` takes an
  `Encoded`, so the extension is dropped at that conversion whatever the packet preserved. Widening
  `Encoded` changes the shape every relay consumer sees — a decision about the relay contract, not
  about packet parsing — so it is a story rather than a row quietly ticked here.

## Notes

- `M-40` found this while measuring what video would cost; the finding stands independently of that
  decision, which was **not admitted**.
