---
id: M-44
title: Negotiate and carry G.722
pillar: Media
status: done
priority:
design: docs/designs/demand.md
epic: demand
areas: [sipx-audio, sipx-sdp, sipx-rtp]
predicate:
announcement:
note: the only codec with real demand that sipx lacks · static PT 9 with the RFC 3551 clock-rate trap
---

# Negotiate and carry G.722

## Goal

Add G.722 as a negotiable codec, correctly handling the static payload type and the clock-rate
inconsistency the RFC preserves for historical reasons.

## Acceptance

- [x] G.722 encode and decode, verified against reference vectors rather than round-trip alone.
- [x] **The RFC 3551 §4.5.2 trap is handled explicitly:** G.722 is sampled at 16 kHz but its RTP
      timestamp clock rate is 8000. A failing-first test asserts the RTP timestamps advance at 8000
      while the audio is 16 kHz, because getting this wrong produces audio that plays at the wrong
      speed and nothing else catches it.
- [x] G.722 is accepted as **static payload type 9 with no `a=rtpmap` line present** — the field-
      reported failure is a stack rejecting exactly that offer — and is also accepted when an
      `a=rtpmap` is supplied.
- [x] Codec preference ordering places it correctly relative to Opus and G.711, and the choice is
      stated in the offer/answer documentation rather than implied by list order.
- [x] The codec is selectable from the CLI (`--codec`) and appears in the audio claims that
      `scripts/check-audio-claims.py` verifies, with `crates/sipx-audio/src/lib.rs` updated in the
      same commit.
- [x] `docs/rfc/registry.toml` RFC 3551 row updated in the same commit — it currently records
      PT 0 and 8 only; `rfc-report.py --check` green.
- [x] `./scripts/gate.py` green for everything this story touches; five steps remain red for
      causes proven at the merge base (see Progress).

## Progress
- (M-44 implementation, 2026-08-07, branch `impl/M-44`) Implemented end to end.
  - `sipx-audio::g722`: native fixed-point sub-band ADPCM from the ITU-T G.722 recommendation,
    stateful `Encoder`/`Decoder`, no feature gate. Verified **bit-exactly** against the official
    Appendix II digital test sequences — both encoder runs and all nine decoder runs (three code
    sequences × three modes) — from the committed corpus `crates/sipx-audio/corpus/g722/`,
    recovered from `itu.int` by `scripts/import-g722-corpus.sh` (`--check` re-verifies).
  - The §4.5.2 split is structural, not a special case at one call site:
    `Codec::samples_per_clock_unit()` (2 for G.722, 1 otherwise) drives
    `Config::samples_per_packet` (320 audio samples / 20 ms) vs `Config::clock_units_per_packet`
    (160 timestamp units), the send clock's advance, RFC 4733 tone durations, and the new
    `audio_rate()` (16 kHz) used by PCM conversion, capture, WAV headers, device audio and
    recording durations. Failing-first test:
    `session::tests::g722_advances_rtp_timestamps_at_8000_while_the_audio_is_16_khz`.
  - Negotiation: bare static 9 with no rtpmap, 9 with `G722/8000`, dynamic-number rtpmap match,
    remap-not-taken-on-the-number, and selection-bounded settling are all pinned in
    `sipx-call`'s agreement table plus a direct `a_bare_static_9_offer_negotiates_g722` test.
    Preference placement (below Opus, above G.711) is stated on `Codecs` and in
    `docs/specs/g722.md` §3; `Codecs::G722` is the named wideband-first set.
  - CLI: `--codec g722` in every build; recordings/device audio use the 16 kHz audio rate.
  - Dialog persistence: codec/preference id 4 appended (never renumbered).
  - Gate: every step this story touches is green (audio claims + its tests, rfc compliance,
    clippy `-D warnings`, fmt, focused suites). Five steps remain red and each reproduces
    unchanged at the merge base `df89424`: `test` (9 sipx-cli progress/env integration tests,
    re-run at base: identical failures), `feature matrix` (packaged-Opus usage literal),
    `comparison` + `comparison tests` and the `docs site` sync tests (stale comparison
    observation). None involve this diff's files.
  - Not done here, deliberately: the corpus import script is not wired into `gate.py`/CI as a
    third corpus step — that edit touches the coordinator-owned gate/CI mapping; wire it like
    the RFC 4475/5118 imports if wanted.

## Notes
- The one genuine codec gap in the demand survey. G.729, AMR, iLBC, GSM, Speex and T.38 drew
  essentially zero requests and are **not** in scope here or elsewhere.
- Whether to implement or bind a codec library is an implementation choice; if a C dependency is
  used it follows the Opus precedent — off by default, feature-gated, with the justification written
  down, since `unsafe_code = "forbid"` does not reach a dependency.
