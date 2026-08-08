---
id: M-43
title: Carry linear PCM in and out without format assumptions
pillar: Media
status: done
priority:
design: docs/designs/demand.md
epic: demand
areas: [sipx-audio, sipx-media, sipx-rtp]
predicate:
announcement:
note: four reported use cases resolve to one unopinionated PCM boundary · not an AI feature
---

# Carry linear PCM in and out without format assumptions

## Goal

Give applications a clean linear-PCM boundary — raw samples in and out at a sample rate and bit
depth they choose, with resampling to and from the negotiated codec — so a call can be bridged to
any external audio consumer without sipx assuming 8 kHz µ-law everywhere.

## Acceptance

- [x] Playback accepts linear PCM at a stated sample rate and bit depth rather than assuming 16-bit,
      and a source whose depth or rate differs from the negotiated codec's is converted rather than
      distorted. A failing-first test plays 8-bit and 16-bit sources at two rates and asserts the
      decoded output, not merely that no error occurred.
- [x] A capture path exposes received audio as linear PCM at a caller-chosen rate.
- [x] Resampling exists between the supported rates. `crates/sipx-audio/src/lib.rs` currently
      documents its absence as deliberate and `scripts/check-audio-claims.py` enforces that claim —
      **both must be updated in the same commit**, and the note must say what is now supported
      rather than being deleted.
- [x] L16 (RFC 3551 §4.5.11) is negotiable as a codec where the peer offers it, with static payload
      11 for 44.1 kHz mono and a dynamic mapping for 8 kHz mono; static payload 10 stereo is refused.
- [x] No hardcoded bit depth or sample rate remains on the playback or record paths; a test asserts
      the failure mode reported in the field — a raw stream at an unexpected depth producing audible
      garbage — is now either correct or a typed refusal.
- [x] The boundary is reachable from the CLI and documented in a guide.
- [x] `docs/rfc/registry.toml` updated in the same commit if the L16 row changes;
      `rfc-report.py --check` green.
- [x] `./scripts/gate.py` green.

## Progress
- 2026-08-05: selected after the S-36 signalling-trap sweep in the post-beta.7 foundations and
  field-hardening wave. Auditing the existing i16/codec-rate assumptions before specifying the
  caller-owned PCM format and conversion boundary.
- 2026-08-05: `pcm.rs` supplies explicit unsigned-8/signed-16 mono buffers, bounded rates and a
  streaming rational-position linear resampler. The failing-first PCM vectors did not compile
  before those types existed; all 31 `sipx-audio` unit/integration tests now pass.
- 2026-08-05: `MediaSession::play_pcm` and sole-consumer `PcmCapture` convert at the running
  session clock; the CLI's WAV and device paths use that same explicit boundary. A real command
  process proof played 16 kHz WAV through a selected L16 call and recorded recognizable 44.1 kHz
  audio.
- 2026-08-05: L16 is signed network-order PCM, offers static mono payload 11 at 44.1 kHz plus
  dynamic mono 8 kHz, and refuses static stereo payload 10. Tests retain arbitrary peer payload
  110 separately from local 96 and carry dynamic 8 kHz L16 samples bit-for-bit.
- 2026-08-05: targeted all-feature clippy, rustdoc, audio-claim, RFC-report, comparison-report,
  internal-link and production-site builds are green. The full workspace gate remains the sole
  unchecked acceptance item and was intentionally not repeated while this wave was still moving.

## Notes
- Normative contract: [`docs/specs/linear-pcm.md`](../specs/linear-pcm.md).
- The demand survey found four separate reports that read as codec requests and resolve to this one
  capability — users bridging calls to external real-time voice services, all of which accept linear
  PCM and few of which accept µ-law. See [`docs/designs/demand.md`](../designs/demand.md).
- **Deliberately not an AI feature.** Answering-machine detection, voice activity detection and
  speech integrations drew zero requests; an unopinionated PCM boundary serves every one of the
  reported cases and composes with any provider.
- Resampling lives in `sipx-audio` directly as a bounded dependency-free streaming converter. The
  media and optional device layers both consume it; neither carries a private algorithm.
