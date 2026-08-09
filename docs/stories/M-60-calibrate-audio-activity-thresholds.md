---
id: M-60
title: Calibrate and adapt audio-activity thresholds deterministically
pillar: Media
status: in-progress
priority: 15
design: docs/designs/call-audio-analysis.md
epic: call-audio-analysis
areas: [sipx-audio, audio-analysis, vad, m16]
predicate:
announcement:
note: after M-58 and M-59 · bounded adaptation with observable reset and limits
---

# Calibrate and adapt audio-activity thresholds deterministically

## Goal

Keep voice activity useful across quiet and noisy calls with bounded, reproducible calibration whose
state and limits are inspectable rather than hidden in wall-clock behavior.

## Acceptance

- [x] A spec amendment defines calibration samples, update rule, floor, ceiling, maximum movement,
      freeze conditions and reset semantics entirely in sample-count terms.
- [x] Failing-first vectors prove identical threshold evolution and activity events across runs,
      including noise ramps, sudden steps, long silence and alternating near-threshold input.
- [x] Call and direction state are independent and bounded; discontinuity, format change and
      cancellation reset exactly the fields the spec names.
- [x] Applications can inspect the active profile and effective thresholds without mutating internal
      state or receiving raw retained audio.
- [x] Adaptation cannot convert clipping, DC or a single impulse into an unbounded active interval,
      and arithmetic cannot overflow for any supported sample.
- [ ] X-106's corpus records the calibrated and fixed-profile result separately and the full gate is
      green.

## Progress

- 2026-08-08: implemented, except the `X-106` row.

- **The amendment is `docs/specs/call-audio-processing.md` §12**, and it is the contract of record.
  §12.1 says exactly one value adapts — `activation_amplitude` — and that §5.3's predicates are
  untouched, so an adapted threshold is a *declared value* in that field's own domain rather than a
  multiplier applied to a fact. §12.2 gives the statistic calibration measures,
  `deviation = isqrt(W·energy − sum²) div W`, with the proof that `V >= A²·W² ⟺ deviation >= A`:
  that equivalence is the whole reason calibration can move a threshold in the same units as the
  fact it governs, and it is why §5.3 needed no change at all. §12.3 to §12.8 are the sample-count
  parameters the story names, §12.9 the inspection boundary, §12.10 the bounds, §12.11 what
  calibration cannot do, and §12.12 the 20 `CAL-*` vectors. §4's "divisions appear only in the
  duration derivation" sentence is amended honestly rather than quietly: the per-window square root
  is now named there, and it is still not in the per-sample path.

- **Everything is a sample count or an amplitude, and the reference profile `K8` is:**
  `calibration_ms = 200` → **C = 1,600 samples** at 8 kHz (the warm-up before the first update may
  apply); `update_ms = 100` → **U = 800 samples** (the update period, so five 20 ms windows);
  `margin_amplitude = 512`; `floor_amplitude = 256`; `ceiling_amplitude = 8,192`;
  `max_step_amplitude = 128` per period; `freeze_limit_ms = Some(30,000)` → **F = 240,000 samples**.
  The update rule is `target = clamp(observed + margin, floor, ceiling)` then
  `next = A + clamp(target − A, ±max_step)`. All four durations derive through §4's existing
  `ceil(d·rate/1000)` and re-derive at every accepted format change, so nothing reads a clock and
  `check-fixed-sleep.py` stays green (43 clock-decided assertions, all classified, unchanged).

- **Period boundaries are a function of sample position alone, never of content** (§12.4). That one
  decision is what makes the vectors readable: an update either happens at sample 800·(p+1) or is
  frozen there, and which it was is a named outcome rather than a reconstruction.

- **Three freeze conditions, tested in order: `Warmup`, `Voiced`, `NoMeasurement`** (§12.6). The
  second is bounded in samples, which is the part that matters. A `Voiced` freeze that could hold
  forever is how a sudden step in noise latches voice open permanently — the fixed profile's
  behaviour — so `freeze_limit_ms` forces the period's own update through after `F` samples. Under
  continuous noise at deviation `d` the forced update moves toward `d + margin`, which is above `d`,
  so the windows go inactive and the hangover closes the interval. CAL-3 pins that end to end:
  `VoiceStarted { 1,600 }`, four forced 1,024-steps at 3,200 / 4,800 / 6,400 / 8,000, then
  `VoiceEnded { at_sample: 8,000, cause: Hangover }`.

- **A window §5.3 already called distortion is not a measurement of background noise.** Clipping,
  impulsive and DC windows are ineligible calibration input (§12.4), and the floor is the *minimum*
  eligible deviation rather than a mean, so one loud outlier cannot drag it either way. CAL-5a/b/c
  prove a stuck full-scale capture, a DC bias and a single impulse move nothing — and that they open
  no interval at all, at any threshold, because §5.3 excludes them before the activation comparison
  is reached. CAL-9 proves the stronger form directly: every window adaptation makes `active` is one
  the *fixed* profile at `floor_amplitude` also makes active, so adaptation is never more sensitive
  than its declared floor. That invariant is unconditional because an `activation_amplitude` outside
  `[floor, ceiling]` is **refused rather than clamped** (CAL-C3), so there is no epoch during which
  it does not hold.

- **Reset names its fields, and one rule covers all three causes** (§12.8). Cleared: the period
  counter, the period's measurement and active flag, the freeze run counter, the readable outcome
  and observed floor, and the warm-up, which is re-armed. Preserved: the effective threshold, the
  calibration profile (re-derived against a new rate on a format change), and the update count. The
  timeline broke; the room did not — discarding a learned threshold at every `Realign` would make a
  lossy network permanently re-calibrate. Cancellation needs no rule of its own: the analyser is
  dropped with the call, so a cancelled call leaves no calibration behind by construction, and
  `teardown_leaves_the_reset_calibration_state` pins that the last readable state is the terminal
  reset's.

- **Inspection is `AudioAnalyzer::thresholds()` and `Call::voice_thresholds()`**, both `&self`, both
  returning a `Copy` `EffectiveThresholds` — a value, not a view. It carries the configured profile,
  the activation amplitude in force, the derived `W`/hangover/silence-timeout/`C`/`U`/`F` counts, the
  update count, the last observed floor and the last outcome. It carries **no audio and there is
  none to carry**: §3.3 forbids retaining samples and §8.1 enumerates the whole of analyser state
  without a buffer in it, so the privacy boundary is a property of the state rather than of a filter
  over it. CAL-11 and `inspecting_thresholds_changes_no_event` prove reading changes nothing an
  observer can see. The call side publishes through a latest-value channel, so an application asking
  what a call is measuring against never blocks the audio path and never gets a history it did not
  ask for.

- **The failing-first vectors are `crates/sipx-audio/tests/call_audio_calibration.rs`**, 21 tests
  replaying §12.12. At the merge base (`ae65b5b`) the file does not compile — `no CalibrationProfile
  in analysis`, `no CalibrationOutcome in analysis`, `no window_deviation in analysis`, and
  `no method named with_calibration found for struct AnalysisProfile` — 32 errors, exit 101. The
  four the story names by shape are CAL-1 (long silence), CAL-2 (a noise ramp, tracked at exactly one
  margin for 120 periods and stopped at the ceiling), CAL-3 (a sudden step) and CAL-4 (alternating
  near-threshold input, at and one below the threshold). CAL-8 replays all four into two analysers
  from one profile and asserts byte-identical drains, thresholds included.

- **Independence is per analyser, and an analyser is per call and per direction.** CAL-7 runs an
  inbound and an outbound analyser to two different thresholds (1,408 and 1,512) from one starting
  point, and each refuses the other's frames outright (`DirectionMismatch`, §7.3), so there is no
  path by which one floor could reach the other. `two_simultaneous_calls_calibrate_independently`
  runs the same shape through the call layer.

- **Overflow is proven, not assumed.** §12.5's largest intermediate is 65,535 over `i32`; §12.2's is
  the `u64` square root of a quantity §5.2 already bounds at `2^62`. §5.2's width proof is untouched
  because the effective threshold never leaves the domain that proof assumed. CAL-10 runs the
  extremes: 65,280-sample windows at 384,000 Hz of alternating `i16::MIN`/`i16::MAX`, then `±32,766`
  against a 32,767 ceiling and a 32,767 margin, with the debug assertions live.

- **`X-106`'s row stays open, deliberately.** It needs a corpus that records the calibrated and the
  fixed-profile result separately, and `X-106` is `backlog` — the corpus, its provenance and its
  budgets are that story's contract and inventing them here would be inventing the evidence. The
  surface it needs now exists: `AnalysisProfile::with_calibration` makes "calibrated" and "fixed"
  two configurations of one analyser rather than two code paths, and `EffectiveThresholds` is how a
  run records which one it measured under. The full-gate half of that row is the wave gate.

- CHANGELOG sentence for the coordinator to place: *"Voice-activity thresholds can now calibrate
  themselves against a call's own background noise, under bounds declared entirely in sample counts
  — a floor, a ceiling, a maximum movement per update period, a warm-up and a bounded freeze while
  voice is open — with every move announced as a typed observation and the effective thresholds
  readable from `AudioAnalyzer::thresholds` and `Call::voice_thresholds` without mutating anything
  or exposing audio (`docs/specs/call-audio-processing.md` §12)."*
