---
id: M-13
title: Add the Opus codec
pillar: Media
status: ready
priority: 11
design: docs/designs/media.md
epic: depth
areas: [sipx-audio]
note:
---

# Add the Opus codec

## Goal
Opus, so a call can sound better than a telephone from 1972 when both ends support it.

## Acceptance
- [ ] Encode and decode Opus at the sample rates SDP negotiates.
- [ ] Negotiated as a dynamic payload type, matched by encoding name — the `M-1` rule, which
      exists precisely for cases like this.
- [ ] G.711 stays the fallback and the negotiation still prefers what the offerer asked for.
- [ ] The added dependency is justified in the story and passes `cargo-deny`.
- [ ] Failing-first test: `an_opus_call_carries_audio_that_survives_the_round_trip`.

## Progress
- Not started.
