---
id: M-3
title: Implement G.711 and WAV handling
pillar: Media
status: backlog
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
- [ ] µ-law and A-law encode and decode, checked against the ITU-T G.711 reference tables
      rather than against a round trip.
- [ ] A round trip through the codec is bit-exact for values the codec can represent.
- [ ] WAV read and write for 8 kHz 16-bit mono, the format the tests use.
- [ ] RFC 4733 DTMF events are encoded and decoded.
- [ ] Failing-first test: `ulaw_matches_the_itu_reference_table`.

## Progress
- Not started.

## Notes
- Checking a codec only by round-tripping proves the two halves agree, not that either is
  right. Use the published tables.
