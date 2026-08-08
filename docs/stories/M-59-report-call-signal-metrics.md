---
id: M-59
title: Report call signal level clipping and silence metrics
pillar: Media
status: in-progress
priority: 14
design: docs/designs/call-audio-analysis.md
epic: call-audio-analysis
areas: [sipx-audio, sipx-call, app-sdk, audio-analysis, metrics, m16]
predicate:
announcement:
note: after M-57 and M-54 · signal content only, distinct from M-10 network quality
---

# Report call signal level clipping and silence metrics

## Goal

Expose deterministic energy, level, clipping and silence-window observations for call audio while
keeping them distinct from packet-loss, jitter, round-trip and MOS statistics.

## Acceptance

- [x] Failing-first sample vectors pin the exact energy/level units, window boundaries, clipping
      definition and silence transition for each supported PCM format and rate.
- [x] Typed observations carry call, direction, sequence, sample time and window coverage through
      `CallEvent` and generated SDK bindings.
- [x] Discontinuity, format change, reset and cancellation produce documented window behavior and
      cannot report samples from an earlier call or format.
- [x] Metric events use a bounded cadence/coalescing policy and cannot block RTP or grow with call
      duration.
- [x] Names and documentation explicitly separate signal-content metrics from M-10's RTP/RTCP
      network-quality snapshot; no existing field changes meaning.
- [ ] Property tests cover extreme samples without overflow or panic and the full gate is green.

## Progress

- 2026-08-08: **implemented.** Three layers, and the first one is deliberately the smallest.

  - **`crates/sipx-audio/src/signal.rs` computes nothing from samples.** The processing contract's
    §10 assigns this story exactly one job — *"shapes level/clipping/silence reporting from `Window`
    facts"* — so `SignalReporter` is a pure reducer over the observations `M-58`'s `AudioAnalyzer`
    already produced. It owns no accumulator over audio, no window, no threshold and no predicate,
    and it does not re-derive §4's duration conversion: the window length is passed in from
    `AudioAnalyzer::window_samples`, because two derivations of one number is how they start
    disagreeing. One observation in, at most one out.
  - **`crates/sipx-call/src/signal_metrics.rs` is the sibling of `voice.rs`** — same seam, same
    analyser contract, same call-owned lifecycle stopped and joined before `Ended`, same bounded
    delivery — differing in one thing: a metric is not a latched state, so there is no reserved slot
    and no retry. A consumer that misses a report has lost history, not correctness.
  - **The wire gains `call.signal.metrics` and `call.signal.silence`** (`app-contract.md` §5.3).
    Additive under §4, and the raw accumulators stay in process deliberately: the wire carries the
    derived facts an application acts on, and adding one of the others later is a field addition
    both sides already have to tolerate.

  Decisions worth carrying forward:

  - **`epoch` is what makes the coverage claim checkable.** Every report names the measurement run
    and rate it was measured in, and a period is discarded unreported if anything could have holed
    it — a reset, a non-contiguous window index, or the analyser's `Lost` marker. A report summing
    across a gap would claim coverage nobody measured, which is worse than no report.
  - **Unmeasured audio is owed forward as a break.** A frame this join cannot forward, or one the
    analyser refuses, sets a pending `Loss` that the next frame carries. Without it the samples
    would silently vanish from the middle of a period. This is the quantitative half of the same
    rule `M-58` applies qualitatively, and it is why the refusal site is not a silent discard.
  - **`rms` is the only derived number, and it is integer.** `floor(sqrt(floor(Σs² / samples)))`,
    in sample amplitude — deliberately not an ITU-T P.56 active speech level, and no floating point
    anywhere in the path.
  - **`active_windows` is carried but no transition is derived from it.** The per-window `active`
    fact is one of §5.3's five over the same accumulators; start, end and hangover stay `M-58`'s.

  The last acceptance row stays open, and only its second half is the reason. The property tests
  exist and pass: `crates/sipx-audio/tests/signal_metrics_properties.rs` drives the corner of §5.2's
  width proof — the widest window the domain admits, filled with the most negative representable
  sample, repeated to a coalescing cadence — and a debug build panics on overflow, so those tests
  either pass or abort rather than wrapping quietly. What is not proven here is *the full gate*,
  which this story did not run: `crates/sipx-transport/tests/discards.rs` is red on the branch this
  landed on, at two sites `M-58` left unexplained (`sipx-call/src/event.rs`'s `try_emit` discard and
  `voice.rs`'s analyser-refusal log). Both are outside this diff and neither is fixed here.

  Left for later: `M-60`'s calibration is unaffected — an adapted threshold is a new
  `AnalysisProfile`, and the reducer summarises whatever windows result.
