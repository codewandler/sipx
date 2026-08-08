---
id: M-75
title: Preserve RTP header extensions on re-encode
pillar: Media
status: ready
priority: 32
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

- [ ] A packet parsed with header extensions and re-encoded emits the same extension bytes, and a
      failing-first test asserts byte equality across the round trip.
- [ ] Dropping an extension, where that is the correct behaviour, is a stated decision in the
      packet's own documentation rather than an omission — say which extensions are forwarded, which
      are rewritten and which are refused.
- [ ] The bridge and conference relay paths are covered, since those are where the loss is
      observable.
- [ ] Malformed or over-long extension data is refused rather than truncated, and a fuzz-shaped
      negative proves it cannot panic.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `M-40`'s adjacent findings — `crates/sipx-rtp/src/packet.rs` parses header
  extensions on receive and drops them on re-encode. Harmless for the audio paths shipping today,
  which do not forward extensions, and a correctness trap for any relay expected to preserve them.

## Notes

- `M-40` found this while measuring what video would cost; the finding stands independently of that
  decision, which was **not admitted**.
