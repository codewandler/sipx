---
id: M-64
title: Attach bounded DSP graphs to calls
pillar: Media
status: in-progress
priority: 13
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-call, sipx-media, dsp, realtime, m18]
predicate:
announcement:
note: after M-54 and M-63 · ordered per-direction graphs, atomic replacement and teardown barrier
---

# Attach bounded DSP graphs to calls

## Goal

Run validated ordered processor chains on transmit and receive PCM without partial graph updates or
one call's DSP delaying another call.

## Acceptance

- [x] Calls attach separate ordered transmit/receive graphs through M-54; the complete graph is
      validated before an atomic activation or replacement.
- [x] Queue, frame, scratch, retained tail and processor-count limits are explicit and non-zero;
      configuration cannot create an unbounded chain or delay line.
- [x] Each frame sees exactly one graph generation, and bypass/removal/replacement has a typed
      sample-boundary transition with no mixture of old and new parameter state.
- [x] Slow or failed processors follow M-63's minimum configured failure policy without awaiting
      application work or stalling RTP on proven-inline/supervised-isolated profiles, and
      discontinuity reaches every downstream processor.
- [ ] Supervised external workers use bounded request/result channels; deadline, crash or malformed
      result applies the declared action, and cancellation terminates and reaps the worker process.
- [x] Drop/cancel/call teardown waits on an observable barrier proving zero graph tasks, frames and
      processor state; concurrent calls share no mutable DSP state.
- [ ] Failing-first live-call tests, feature combinations, strict clippy and the full gate are green.

## Progress

- 2026-08-09: implemented, with one Acceptance row deliberately left open. The normative document is
  [`docs/specs/call-dsp-graph.md`](../specs/call-dsp-graph.md); the code is
  `crates/sipx-media/src/dsp/` (`mod.rs`, `graph.rs`, `supervised.rs`) plus the wiring in
  `crates/sipx-media/src/session.rs`. Live-call tests are
  `crates/sipx-media/tests/dsp_graph.rs` (12) and `dsp::graph::tests` (6).

  **Where a graph runs, and why it is not a second tap.** The chain runs *in path*, at `M-54`'s two
  tap points and immediately before the seam offers the frame — outbound after the mute gate and
  before encoding, inbound at the jitter buffer's output after decode. Running it before the offer
  is what keeps [`call-audio-seam.md`](../specs/call-audio-seam.md) §3's own sentences true rather
  than newly false: the outbound tap still reports the samples that actually become RTP, and the
  inbound tap still reports what `MediaSession::recv` delivers. A seam consumer does not have to
  know whether a graph is attached to be right about what it heard.

  **What attaching promises, per profile, and what it does not.** `DspGraph::contains_overrun()` is
  the conjunction of the stages' profiles and nothing else: a chain of supervised and proven-inline
  stages may claim that over-budget work in it cannot stall RTP, and **one**
  `TrustedCooperativeNative` stage makes the whole chain unable to claim it, because that stage is
  application code on the media worker that sipx cannot preempt, cancel or reap. `M-63`'s rule that
  `contains_overrun()` is false unconditionally for that profile is inherited, not re-decided.
  `ProvenInline` is admitted only through a crate-internal door (`GraphPlan::from_workspace`); the
  public `GraphPlan::with_processor` refuses it by type, because "proven" names evidence in this
  repository's gate and an application cannot add to it. The workspace registry is empty until
  `M-65` ships processors for it, so today that refusal is total — which is the correct state of a
  workspace that has proven nothing yet.

  **How the no-I/O and no-spawn rules stay true on the live path.** Every buffer, every channel slot
  and every supervised worker is created when the plan is validated. On the media worker the graph
  takes one lock, swaps two `Vec`s rather than copying, runs the chain, and returns: no await, no
  allocation, no clock read, no spawn. A deadline is counted in frame durations rather than measured
  against a clock, so nothing in the runtime reads one on a processor's behalf — a processor that
  could read its own deadline would have a clock.

  **Teardown.** `GraphBarrier` reports workers, frames in flight, retained positions and processors,
  and is clear only when all four are zero. `stop()` and `Drop` are the synchronous half — every
  stage cancelled, every worker told to finish; `shutdown()` and `DspGraph::detach()` are the same
  teardown with the reap awaited. The reap waits on an event (`session::Stop`, reused) and never a
  duration, which is `M-93`'s rule: a stopped session's graph answers rather than holding a runtime
  worker nothing can reclaim.

  **What is not done, and why the row is unticked.** The supervised profile's worker runs in a
  thread `sipx-media` owns rather than in an operating-system process. Everything about the profile
  that is a protocol *is* implemented and tested — bounded request and result channels, the deadline
  as a pipeline depth, the miss budget, and `DeadlineMissed` / `MalformedResult` / `WorkerLost`
  applying the declared action, with a panicking worker caught and reaped — but a thread cannot be
  killed, so the sentence "cancellation terminates and reaps the worker **process**" is not
  delivered. `M-102` is filed for it and the limit is stated in
  `crates/sipx-media/src/lib.rs`'s stability section and in `call-dsp-graph.md` §7 rather than
  papered over. Reaching it inside this story would have meant adding `tokio`'s `process` feature or
  a `[[bin]]` worker target to `crates/sipx-media/Cargo.toml`, which is a manifest dependency edit
  this story was fenced from.

  **Two deliberate narrowings, both written into the spec rather than assumed.** §4.3: a stage
  declaring `LengthPolicy::Bounded` is refused for a live attachment, because the session's
  packetisation is fixed and this story defines no re-framing stage — the processor contract is
  unchanged, only what a *live call* admits. §5.4: a replacement discards the outgoing chain's
  retained tail rather than flushing it into the incoming one's first frame, because that tail is
  audio at positions in an epoch that no longer exists.

  **Owed CHANGELOG sentence** (not written here — the ledger is the coordinator's):
  *Added: bounded, ordered DSP graphs attach to either direction of a live call
  (`MediaSession::attach_dsp`, `sipx_media::dsp`). A chain is validated whole before an atomic
  activation or replacement, every frame sees exactly one generation, a slow or failing processor
  costs its configured fail-open or fail-closed action rather than the call's RTP, and teardown
  reports an observable barrier proving the graph holds nothing. Only chains built entirely from
  proven-inline and supervised-isolated stages claim that over-budget work cannot stall RTP; the
  supervised worker runs in a thread rather than a separate process until `M-102`.*
