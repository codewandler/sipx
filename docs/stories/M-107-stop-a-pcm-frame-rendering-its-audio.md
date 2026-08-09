---
id: M-107
title: Stop a PCM frame rendering its audio
pillar: Media
status: ready
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

- [ ] A failing-first test renders a `PcmFrame` and asserts no sample value appears in the output,
      the way `M-61`'s `a_frame_diagnostic_carries_counters_and_no_raw_audio` does for its own type.
- [ ] The hand-written `Debug` carries direction, position and a sample count, and its length is
      bounded independently of the frame's — an unbounded diagnostic is the second half of the same
      defect.
- [ ] Every other public type on the media surface holding a sample buffer is checked for the same
      derive, and each is either fixed or named with the reason its rendering is safe. A fix for one
      type and a shrug for its siblings is how this recurred in the first place.
- [ ] Whether this can be *enforced* rather than reviewed is answered: `check-audio-claims.py`
      already reads this surface, and a derive that prints a sample buffer is a shape a checker can
      look for. If it cannot be checked, say why.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-09: filed at integration from `M-61`'s adjacent findings. Its own fix is the model, and
  its report is the evidence that the shape is reachable in practice rather than in principle.
