---
id: M-13
title: Add the Opus codec
pillar: Media
status: done
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
- [x] Encode and decode Opus at the sample rates SDP negotiates.
- [x] Negotiated as a dynamic payload type, matched by encoding name — the `M-1` rule, which
      exists precisely for cases like this.
- [x] G.711 stays the fallback and the negotiation still prefers what the offerer asked for.
- [x] The added dependency is justified in the story and passes `cargo-deny`.
- [x] Failing-first test: `an_opus_call_carries_audio_that_survives_the_round_trip`.

## Progress
- Done, behind the `opus` feature in `sipx-audio` and `sipx-media`. Off by default, so the
  stack stays pure Rust unless somebody asks for the codec.
- **The dependency, and the one thing about it that is not clean.** `opus 0.3` is
  MIT/Apache-2.0 and its licence passes. Its FFI layer `audiopus_sys` is **unmaintained**
  (RUSTSEC-2026-0150) and `cargo-deny` failed on it — exactly the check this story asked for.
  There is no maintained alternative: the pure-Rust crates decode and do not encode, and a
  codec sipx can decode but not encode is one it cannot offer. The other bindings either wrap
  the same `audiopus_sys` or fail to build.
  So the advisory is excepted, narrowly and with the reasoning written into `deny.toml`. What
  bounds it: the advisory's concrete complaint is a CMake pin that bites only when libopus has
  to be built from source, and CI now installs `libopus-dev` so pkg-config is used instead; and
  Opus is behind a **non-default feature**, so nothing reaches it unless a build asks for the
  codec. **This is a judgement call and worth a second opinion** — the alternative, shipping no
  Opus at all until a maintained binding exists, is entirely defensible.
- Opus forced a real change: it is **stateful**, where G.711 is a pure function of one frame.
  The encoder and decoder now live one each in the send and receive loops, so a stateful codec
  costs no lock — exactly one task ever encodes and exactly one ever decodes.
- Two things that reach up out of the codec. The RTP clock is **48000 whatever the audio rate**
  (RFC 7587 §7), so `Codec::clock_rate` is no longer the sample rate; and Opus has **no static
  payload type at all**, so `Config::payload_type` carries the negotiated number and
  `Codec::from_payload_type` deliberately never returns Opus — guessing it from 111 would decode
  somebody else's G.729 as Opus.
- The negotiation test that matters is `opus_is_matched_even_when_the_far_end_numbers_it_
  differently`: the offerer calls Opus 96, sipx calls it 111, and the answer uses *the
  offerer's* number. Matching on the number rather than the encoding name would silently drop
  to G.711.
- Correlation at the best lag rather than at zero, in both round-trip tests. Opus has an
  algorithmic delay, so a sample-aligned comparison measures the delay rather than the audio —
  the first version scored 0.48 on a signal that had survived perfectly well. Each test also
  correlates against an *unrelated* tone, so the lag search cannot pass by matching anything.
