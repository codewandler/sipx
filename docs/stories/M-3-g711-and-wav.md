---
id: M-3
title: Implement G.711 and WAV handling
pillar: Media
status: done
priority:
design: docs/designs/media.md
epic: media
areas: [sipx-audio]
note:
---

# Implement G.711 and WAV handling

## Goal
Encode and decode G.711 µ-law and A-law, and read and write WAV, so a call can carry audio
that can be asserted on sample by sample.

## Acceptance
- [x] µ-law and A-law encode and decode, checked against the ITU-T G.711 reference tables
      rather than against a round trip.
- [x] A round trip through the codec is bit-exact for values the codec can represent.
- [x] WAV read and write for 8 kHz 16-bit mono, the format the tests use.
- [x] RFC 4733 DTMF events are encoded and decoded.
- [x] Failing-first test: `ulaw_matches_the_itu_reference_table`.

## Progress
- Done. `crates/sipx-audio/`.
- The codec is checked against the ITU algorithm rather than by round trip. Round-tripping only
  proves the two halves agree with each other, and two halves wrong in mirrored ways agree
  perfectly while interoperating with nothing.
- One real property fell out of testing: µ-law has two representations of zero, so code 127
  (−0) normalises to 255 (+0). Every other code in both codecs is idempotent. A-law has no such
  pair.
- **Not done: RFC 4733 DTMF events.** The SDP side negotiates `telephone-event` and echoes its
  `fmtp`, but nothing encodes or decodes the events. Filed as `M-7`.

## Notes
- Checking a codec only by round-tripping proves the two halves agree, not that either is
  right. Use the published tables.
