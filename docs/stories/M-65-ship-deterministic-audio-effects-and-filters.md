---
id: M-65
title: Ship deterministic audio effects and filters
pillar: Media
status: in-progress
priority: 39
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-audio, dsp, filters, effects, m18]
predicate:
announcement:
note: after M-63 · gain/filter/distortion/bit-crush/stutter processors use the public contract
---

# Ship deterministic audio effects and filters

## Goal

Provide a practical built-in effect set without giving those processors private access to the media
runtime.

## Acceptance

- [x] Built-ins include gain, polarity, hard/soft clipping, bit crushing, bounded delay/stutter
      glitching, and stable high-pass, low-pass and peaking filters.
- [x] Every processor publishes closed parameter ranges, supported rates/channels, latency, tail,
      reset and smoothing behavior through M-63 capability discovery.
- [x] Golden impulse, step, frequency and sample vectors prove exact ordering, saturation behavior,
      chunk-boundary independence and deterministic parameter transitions.
- [x] Extreme amplitude, invalid coefficients/rates, zero/maximum delay and arbitrary finite PCM
      never produce NaN, wraparound, panic or allocation beyond the declared bound.
- [x] Intentional glitch/stutter output is distinguishable in events/metrics from accidental
      discontinuity or deadline miss; docs never present an overload defect as an effect.
- [ ] Each built-in passes the external-processor conformance harness and the full gate is green.

## Progress

- 2026-08-09 — Nine processors ship in `crates/sipx-audio/src/dsp/effects/`, each an ordinary
  `FrameProcessor` built from the public contract and nothing else: `sipx.gain`, `sipx.polarity`,
  `sipx.hard_clip`, `sipx.soft_clip`, `sipx.bit_crush`, `sipx.stutter`, `sipx.low_pass`,
  `sipx.high_pass`, `sipx.peaking`. `docs/specs/call-dsp-effects.md` is the normative document: each
  effect's arithmetic to the last bit, and a normative "does not do" paragraph beside it.

- **Failing first.** `crates/sipx-audio/tests/dsp_effects.rs` was written and run at the merge base
  `465605dd4cd04c5fd36ee32913f7d323efa77d22`, where `cargo test -p sipx-audio --test dsp_effects`
  reported `error[E0432]: unresolved import 'sipx_audio::dsp::effects'` and
  `error: could not compile 'sipx-audio' (test "dsp_effects") due to 2 previous errors`. It now runs
  26 tests green, EFFECT-V1 through EFFECT-V19, from **outside** the crate — which is the epic's own
  claim that a built-in reaches the runtime through the same door an application does.

- **Conformance (`DSP-K1`..`DSP-K12`).** Every built-in passes; the `Stutter` is run at lines of 0,
  1, 4 and 4,096. The unproven set is asserted to be **exactly `DSP-K9`** for each of them, which is
  the allocation check §9.1 declares unprovable for any processor because `unsafe_code` is forbidden
  workspace-wide and no counting allocator can be installed. Nothing else comes back `Unproven`:
  declaring two channels rather than the contract's eight is what leaves `DSP-K2` a provable check
  rather than one with nothing admissible to refuse.

- **The registry, and a graph with a real stage.** `sipx-media` grows `dsp::BuiltIn` and
  `GraphPlan::with_built_in`, which is the workspace registry `call-dsp-graph.md` §3.2 said was
  empty. Provenance is recorded **per stage** rather than per plan, so a built-in beside an
  application-supplied processor does not lend it `ProvenInline`; the same built-in offered at the
  public door is still refused `ProfileNotAdmissible`. `GraphError::Parameter` is new, and
  `dsp_graph.rs` proves a `sipx.gain` stage doubling live outbound audio on a real session.

- **Decisions a reviewer should check rather than assume.** (a) `Saturated` is emitted only for a
  clamp to *full scale*, so a hard clipper at a declared ceiling emits nothing — the contract's word
  keeps its meaning and the effect documents the silence. (b) `Gain` declares `Stateless`: its ramp
  is parameter state, which §8.1 keeps across a reset, and it holds no sample memory. (c) The
  filters carry their one-pole state at Q15; at sample resolution the rounded update dead-bands at
  `16384/G`, which is four LSB of permanent DC offset in a 300 Hz high-pass. (d) A cutoff past
  0.45·rate is clamped, not refused, because the parameter domain is closed independently of the
  rate.

- **Gate row left unticked.** `./scripts/gate.py` was not run here — one gate per wave, by the
  coordinator. What was run in this worktree, all green: `cargo test -p sipx-audio -p sipx-media
  --all-features`, `cargo clippy` on both `--all-targets --all-features --no-deps -- -D warnings`,
  `cargo fmt --all`, `cargo doc` on both `--no-deps --all-features` (zero warnings),
  `cargo build -p sipx-audio -p sipx-media --no-default-features`, `check-audio-claims.py --check`,
  `check-fixed-sleep.py --check`, `check-app-surface.py --check`, `check-provenance.sh`,
  `check-story-closure.py`.

- **Owed CHANGELOG sentence** (the coordinator writes it; this story may not touch that file):
  *Nine deterministic built-in call-DSP processors — gain, polarity, hard and soft clipping, bit
  crushing, a bounded delay/stutter line, and one-pole low-pass, high-pass and peaking filters —
  ship through the public processor contract, and `GraphPlan::with_built_in` is the workspace
  registry that lets a call-local graph run them inline.*

- **Left open.** Measured frequency response, CPU and allocation belong to `X-109`; the peaking
  filter's `band_gain` is documented as the scale applied to the extracted band rather than the
  magnitude at the centre frequency, and only a measurement may say otherwise. No SDK-side
  parameter control (`M-67`) and no noise reduction (`M-66`) are attempted here.
