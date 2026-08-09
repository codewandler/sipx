---
id: M-107
title: Stop a PCM frame rendering its audio
pillar: Media
status: in-progress
priority: 5
design:
epic: media
areas: [sipx-media]
predicate:
announcement:
note: PcmFrame derives Debug over its samples · the same shape M-61 fixed one layer down, where it was reachable from a real refusal record
---

# Stop a PCM frame rendering its audio

## Goal

Make `sipx_media::PcmFrame`'s `Debug` carry what a diagnostic needs — identity, position, a sample
*count* — and not the samples themselves.

## Why

`M-61` found and fixed exactly this on `sipx_audio::AnalysisFrame`: the derived `Debug` rendered
every borrowed sample, so any `tracing` field, panic message or test failure carrying a frame put
**raw call audio** into a record whose length was the frame's — up to 65,536 values. It was reachable
from `sipx-call`'s own refusal record in `audio_feed.rs`.

`crates/sipx-media/src/processing.rs`'s `PcmFrame` has the same shape and the same derive. `M-61`'s
implementor reported it and deliberately did not fix it: it belongs to `M-54`'s type, not to that
story, and nothing currently logs it — so it is **latent rather than live**. That is the whole
argument for filing it rather than shrugging: the difference between this and the one that shipped
is a single `tracing::warn!` somebody adds later, and by then it is a privacy defect in a released
crate rather than a derive nobody thought about.

The processing seam's own contract says the analysis path retains no audio and that diagnostics
carry identity and counters. A `Debug` that prints the samples contradicts both, silently, at the
moment something goes wrong — which is precisely when a record gets written.

## Acceptance

- [x] A failing-first test renders a `PcmFrame` and asserts no sample value appears in the output,
      the way `M-61`'s `a_frame_diagnostic_carries_counters_and_no_raw_audio` does for its own type.
- [x] The hand-written `Debug` carries direction, position and a sample count, and its length is
      bounded independently of the frame's — an unbounded diagnostic is the second half of the same
      defect.
- [x] Every other public type on the media surface holding a sample buffer is checked for the same
      derive, and each is either fixed or named with the reason its rendering is safe. A fix for one
      type and a shrug for its siblings is how this recurred in the first place.
- [x] Whether this can be *enforced* rather than reviewed is answered: `check-audio-claims.py`
      already reads this surface, and a derive that prints a sample buffer is a shape a checker can
      look for. If it cannot be checked, say why.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-09: filed at integration from `M-61`'s adjacent findings. Its own fix is the model, and
  its report is the evidence that the shape is reachable in practice rather than in principle.

- 2026-08-09: implemented on `impl/M-107`. Failing-first, then the audit, then the rule.

  **The red.** `a_frame_diagnostic_carries_counters_and_no_raw_audio` in
  `crates/sipx-media/tests/processing.rs` sends 160 samples of 30,011 through an `Outbound`
  attachment and renders the frame the seam hands back. `PcmFrame` has no public constructor, so
  the only frame there is to render is the one an application receives — which is the one whose
  rendering this is about. At `98983cf`:

  ```
  thread 'a_frame_diagnostic_carries_counters_and_no_raw_audio' panicked at
  crates/sipx-media/tests/processing.rs:476:5:
  a frame's diagnostic rendering carries no sample value: PcmFrame { direction: Outbound,
  pcm: Pcm { format: PcmFormat { sample_rate: 8000, encoding: Signed16 },
  samples: Signed16([30011, 30011, … ×160 …, 30011]) }, sample_time: 0, sequence: 0,
  discontinuity: None }
  ```

  **The audit, and what it found under the story's type.** The leak was not `PcmFrame`'s alone.
  `Pcm` renders a `PcmSamples`, and `PcmSamples` is where every owned buffer in the workspace
  actually lives — so the redaction went there, and `Pcm` now derives its `Debug` safely *by
  composition*. Nine reachable public types hold a raw sample buffer; every one now implements its
  own `Debug`. Fixed: `PcmFrame`, `PcmSamples`, `Wav`, `DspFrame`, `Scratch`, `FrameSink`,
  `Stutter`, `g722::Encoder`, `g722::Decoder`, and `Encoded` (encoded payload, by hand — see
  below). Checked and safe with the reason recorded: `Pcm` (composition), `AnalysisFrame`,
  `RecognitionFrame`, `SynthesisChunk` (already redacted), `SrtpKeys` (already `{ .. }`),
  `DspCapability` (`&'static [u8]` channel list, a compile-time constant), `ice::Input`,
  `ice::Output`, `stun::Attribute` (STUN control bytes, classified before they reach the agent),
  and the DSP effects' `[i32]`/`[i64]` Q15 filter accumulators (a per-channel constant, not a
  buffer).

  **Enforceable, at the layer where an element type decides it.**
  `scripts/check-audio-claims.py` gained `sample_buffer_problems`: a reachable public type whose
  declaration names a `Vec<i16>`, `[i16]`, `[i16; N]` or the `f32` equivalents must *implement*
  `Debug` or carry an adjacent `/// Not call audio:` rationale. It runs over every published crate
  with no rollout boundary. It asks for an implementation rather than forbidding the derive
  deliberately: a reader that stopped recognising `#[derive(Debug)]` would excuse every carrier at
  exit 0, where one that stops recognising the implementation reports all nine — the correction
  `M-97` made to `marked`. The selector is the one narrowing that would be quiet, so the population
  is held to `_PLAUSIBLE_CARRIERS` and printed on every run.

  **What it cannot check, and why that is the honest answer rather than a gap.** Encoded audio is
  `Bytes`, which in this workspace is equally a `Call-ID`, a SIP body, a URI and a STUN attribute —
  around sixty reachable public types hold one, and for nearly all of them rendering the bytes is
  what a protocol log is for. So `Encoded` was redacted by hand and `sipx_rtp::Packet`, the same
  payload one layer down and outside this story's crates, is filed as `M-110`.

  `docs/specs/call-audio-seam.md` §4 gained the normative sentence and SEAM-16 gained the vector,
  so the rule is a contract rather than an implementation detail.

  **Owed to `CHANGELOG.md`** (fenced; the coordinator writes it):
  `sipx-media`'s `PcmFrame` and `Encoded`, and `sipx-audio`'s `PcmSamples`, `Wav`, `DspFrame`,
  `Scratch`, `FrameSink`, `Stutter` and the G.722 codecs, no longer render call audio in their
  `Debug`: each reports identity and a sample count, in a record whose length is bounded
  independently of the audio. `check-audio-claims.py` now holds every published crate to it.

  The `gate` row is unticked: the wave gate is the coordinator's. Verified in this worktree:
  `cargo test -p sipx-media -p sipx-audio -p sipx-call --all-features`, `cargo clippy` on those
  `--all-targets --all-features --no-deps -- -D warnings`, `cargo fmt --all`, `cargo doc` on those
  `--no-deps --all-features`, `python3 -m unittest scripts/test-audio-claims.py` (129 tests, 12 of
  them new), `./scripts/check-audio-claims.py --check`, `./scripts/gate.py --check`.
