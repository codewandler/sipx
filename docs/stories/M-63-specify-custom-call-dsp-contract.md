---
id: M-63
title: Specify the custom call-DSP contract
pillar: Media
status: done
priority: 37
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-audio, sipx-media, dsp, m18]
predicate:
announcement:
note: after M-54 · M18 admission · frame contract, execution profiles and minimum failure policy
---

# Specify the custom call-DSP contract

## Goal

Define one deterministic processor interface shared by built-in effects, noise reduction and
application-supplied DSP before any live-call attachment is implemented.

## Acceptance

- [x] A normative spec defines PCM format/channel metadata, direction, sample position,
      discontinuity, finite parameter state, output shape and typed observations.
- [x] Capability discovery declares accepted formats, maximum frame, scratch/state bound,
      algorithmic latency/tail, length preservation and reset/flush behavior.
- [x] The contract names proven-inline, supervised-isolated and trusted cooperative-native execution
      profiles, including deadline action and fail-open/fail-closed policy; only the first two may
      claim that over-budget work cannot stall RTP.
- [x] The processor performs no I/O, clock read, task spawn or device discovery; time is sample
      position/rate input and the same vectors are deterministic under virtual or wall time.
- [x] Invalid format, frame, channel, parameter and discontinuity inputs return typed errors without
      partial state mutation, panic, unsafe code or unbounded allocation.
- [x] A conformance harness accepts an external fixture processor and proves reset, cancellation,
      chunk-boundary and allocation invariants.
- [x] Public API docs, byte/sample vectors and the full gate are green.

## Progress

- 2026-08-08: implemented. The normative contract is
  [`docs/specs/custom-call-dsp.md`](../specs/custom-call-dsp.md), and the types and harness it
  specifies are `crates/sipx-audio/src/dsp/` (`contract.rs`, `conformance.rs`).

  **What the spec now states normatively.** §3 the input contract — direction and discontinuity
  imported from the analysis contract rather than re-minted, an interleaved signed-16-bit
  `StreamFormat` of 1..=8 channels over the linear-PCM rate domain, a borrowed frame, the epoch
  and sample-position timeline taken from the seam's `sample_time`, the §3.5 position table, and a
  parameter vocabulary (`Flag` / `Integer` / `Ratio` in thousandths) with no floating-point variant
  so that "finite parameter state" is unrepresentable rather than validated. §4 determinism: no
  I/O, no clock read, no task spawn, no device discovery, no random source, no shared mutable
  state; floating point restricted to the exactly-rounded IEEE 754 operations with transcendentals
  banned from the sample path; and scratch contents on entry unspecified, so reading unwritten
  scratch is a contract violation the harness catches by poisoning each run differently. §5
  capability discovery: accepted rates and channels, maximum frame, scratch and state bounds,
  algorithmic latency and tail, length policy, reset behaviour, execution policy and the closed
  parameter schema — validated before any caller sizes a buffer from it. §6 output shape and typed
  observations, with the analysis contract's coalescing `Lost` accounting applied unchanged. §7 the
  three execution profiles. §8 reset / flush / cancel / refusal, with a refusal table. §9 the
  memory and CPU bounds, including a written statement of what the allocation bound is *not*. §10
  the one-tap prohibition and the assignment of the rest to `M-64`–`M-68`, `X-109` and `A-34`. §11
  conformance, including §11.3 making it normative that the harness be shown failing. §12 the
  `DSP-V*` vectors.

  **The three execution profiles, and exactly what each claims.** The claim is decomposed into
  `OverrunContainment` and `DeadlineAction` because the two facts are separately true.
  *Proven inline* — `BoundedWork`, `AccountAfterReturn`: claims that over-budget work cannot stall
  RTP because the work is bounded by construction (allocation-free, `O(positions)`, no
  data-dependent loop, declared frame ceiling, harness-checked); explicitly does **not** claim that
  anything preempts a running call, nor that a machine too slow for the graph will meet its
  deadlines — the bound is on work, not wall time. Crate-owned processors only.
  *Supervised isolated* — `BoundedWait`, `AbandonResult`: claims that over-budget work cannot stall
  RTP because `process` never runs on the media worker and the wait is bounded by the deadline;
  explicitly does **not** claim the audio survives (an abandoned result is unprocessed audio, so a
  fail-open bypass is audible) nor that the isolated process's own memory and CPU are bounded
  beyond what the OS is configured to bound.
  *Trusted cooperative native* — `None`, `AccountAfterReturn`: claims **nothing** about containment,
  and says why in full — sipx cannot preempt, cancel, reap or memory-bound it, and a callback that
  does not return stalls RTP for that call because every deadline, failure action and teardown
  barrier is code that runs after it returns. `ExecutionProfile::contains_overrun()` is `false` for
  this profile unconditionally; no configuration or conformance result may set it true, and
  `dsp_v10_only_two_profiles_contain_an_overrun` asserts that.

  **The failure policy** (§7.4) is the minimum `M-64` implements: `deadline_frames` in frame
  durations (never milliseconds — a processor that could read its own deadline would have a clock),
  `max_consecutive_misses`, and `on_failure` as `BypassOpen` (fail open, the call sounds wrong) or
  `TerminateClosed` (fail closed, the audio the processor was protecting does not flow). Neither is
  a default that guesses.

  **The harness** (`Conformance`) accepts any processor from any crate through a factory and runs
  twelve named checks, `DSP-K1`..`DSP-K12`, every one of which appears in every report. A check it
  cannot prove is `CheckStatus::Unproven` with a stated reason and never a pass. It is proved
  against itself: `crates/sipx-audio/tests/dsp_conformance.rs` builds every fixture from the public
  API alone — outside the crate, which is the same sentence as "an application can implement this"
  — and eight of them break exactly one invariant each: `LeakyReset` (`DSP-K7`), `ZombieCancel`
  (`DSP-K8`), `ChunkDependent` (`DSP-K6`), `ScratchHog` (`DSP-K9`), `LengthLiar` (`DSP-K11`),
  `SloppyRefusal` (`DSP-K3`), `Panicky` (`DSP-K12`, caught rather than taking the harness down) and
  `ScratchReader` (`DSP-K5`, caught by the poisoned workspace). `DSP-K3` splices each refusal into
  its own replay rather than batching them, because a batch can hide a mutation a later refusal
  happens to undo — which is exactly what the first draft did, and `SloppyRefusal` passed until it
  was fixed.

  **Failing-first.** `crates/sipx-audio/tests/dsp_conformance.rs` was written and run at the merge
  base (`49c5de5`) before any implementation existed:
  `cargo test -p sipx-audio --all-features --test dsp_conformance` →
  `error[E0432]: unresolved import 'sipx_audio::dsp'` / `could not find 'dsp' in 'sipx_audio'`,
  21 errors, exit 101. It now reports `27 passed; 0 failed`.

  **Left open deliberately.** The last acceptance row stays unticked: it bundles the full gate, and
  the wave gate is run once by the coordinator. `DSP-K9` reports heap growth inside a processor's
  own state as `Unproven` — `unsafe_code` is forbidden workspace-wide, so no counting allocator can
  be installed, and the harness measures the inline size and holds the caller-owned workspace
  exactly instead of reporting a figure it cannot produce. `X-126` is filed for that measurement.

- 2026-08-08: closed at the `1.0.0-rc.11` boundary, against the wave gate run on this tree.
