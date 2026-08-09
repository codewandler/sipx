//! Bounded DSP graphs attached to live calls
//! ([`docs/specs/call-dsp-graph.md`](../../../docs/specs/call-dsp-graph.md), `M-64`).
//!
//! [`sipx_audio::dsp`] defines what one processor is: a synchronous, sans-I/O frame transform that
//! owns no socket, no clock, no thread and none of the memory it works in. This module is what
//! attaches an ordered chain of them to a running call, and it inherits that contract rather than
//! reinterpreting it.
//!
//! Three properties shape everything here.
//!
//! **A graph runs at `M-54`'s tap points and adds no second one.** Outbound, it runs in the send
//! loop after the mute gate and before encoding; inbound, at the jitter buffer's output after
//! decode. It runs *before* the seam offers the frame, so
//! [`crate::processing`]'s own sentences stay true rather than becoming false: the outbound tap
//! still reports the samples that actually become RTP and the inbound tap still reports what
//! [`MediaSession::recv`](crate::MediaSession::recv) sees.
//!
//! **One frame sees one generation, entire.** The graph's state is taken once per frame and
//! released once per frame, so a replacement, a bypass or a teardown is applied *between* frames.
//! There is no frame that saw stage 1 of one generation and stage 2 of the next, and none that saw
//! one stage's old parameters beside another's new ones. Every change is reported as a typed
//! [`GraphTransition`] naming the generation and the position it took effect at.
//!
//! **Teardown is a barrier, not a wait.** Detach, a fail-closed failure, a renegotiation that
//! changed the audio format, `stop()`, `shutdown()` and drop all reach [`GraphBarrier`], and it is
//! clear only when the graph holds zero workers, zero frames in flight, zero retained positions and
//! zero processors. Nothing here is observed by waiting a fixed duration.
//!
//! # What is counted, and what is not (`M-68`)
//!
//! [`GraphTransition`]s carry the detail and their queue is bounded, so it drops its oldest at
//! capacity. [`GraphCounters`] is what survives that: cumulative for the life of the call, across
//! every generation, and still answering after the graph has ended. Nothing a *processor* observed
//! about a frame is in there — a [`DspObservation`] is the processor's own vocabulary, and an
//! intentional glitch effect's `Saturated` is not a defect the runtime counted.
//!
//! A graph is torn down rather than carried over when a `MediaSession::reconfigure` changes the
//! audio format or the packetisation, because both are things every stage was validated, prepared
//! and sized against and `docs/specs/custom-call-dsp.md` §8.3 makes a rate change a `prepare` and
//! never a frame. [`TeardownCause::FormatChanged`] says so; attaching a graph for the format the
//! call now carries is the application's to do.
//!
//! # What attaching a graph promises, per profile
//!
//! A graph is **only as contained as its least contained stage**, and
//! [`DspGraph::contains_overrun`] is the conjunction over its stages. It is not configurable and no
//! measurement changes it:
//!
//! - every stage [`ProvenInline`] or [`SupervisedIsolated`] — over-budget work in this graph cannot
//!   stall RTP. For a supervised stage that is because the media worker never waits for it: it
//!   offers a frame to a bounded channel and takes a result only if one is present by the declared
//!   deadline, and a hang, a crash or a malformed result costs the declared action plus a
//!   termination and a reap. The termination is a kill and the reap is a `wait`, because that stage
//!   runs in an operating-system process this crate spawns and owns (`M-102`) — which is also what
//!   puts its memory and CPU inside whatever the operating system is configured to bound.
//! - any stage [`TrustedCooperativeNative`] — **nothing about containment**. That stage is
//!   application code on the media worker: sipx cannot preempt it, cancel it or reap it, and a
//!   callback that does not return stalls RTP for that call. No deadline, failure action or
//!   teardown barrier changes that, because every one of them is code that runs after the callback
//!   returns.
//!
//! [`ProvenInline`] is admitted only for processors inside this workspace, so
//! [`GraphPlan::with_processor`] — the door an application reaches — refuses it. The registry that
//! does admit it is [`BuiltIn`], a closed set of this workspace's own effects and filters reached
//! through [`GraphPlan::with_built_in`]: what it grants is provenance and not access, so the same
//! processor constructed by hand and offered at the public door is refused exactly as before.
//!
//! # What stays true on the live path
//!
//! The processor contract's §4 obligations are the processor's, and this attachment keeps them
//! reachable rather than quietly relaxing them. On the media worker a graph performs **no I/O, no
//! clock read, no task spawn and no allocation**: every buffer, every channel slot and every
//! supervised worker is created when the plan is validated, and a deadline is counted in frames
//! rather than measured against a clock, because a processor that could read its own deadline would
//! have one.
//!
//! [`ProvenInline`]: sipx_audio::dsp::ExecutionProfile::ProvenInline
//! [`SupervisedIsolated`]: sipx_audio::dsp::ExecutionProfile::SupervisedIsolated
//! [`TrustedCooperativeNative`]: sipx_audio::dsp::ExecutionProfile::TrustedCooperativeNative

use crate::processing::AudioDirection;

mod builtin;
mod graph;
mod supervised;
mod wire;
mod worker;

pub use builtin::BuiltIn;
pub use graph::{
    BypassCause, GraphBarrier, GraphBounds, GraphCounters, GraphError, GraphPlan, GraphTransition,
    MAX_PROCESSORS, MAX_WORKER_QUEUE, TeardownCause,
};
pub use supervised::WorkerProcess;
pub use wire::WorkerProtocolError;
pub use worker::{SupervisedWorker, WorkerResult, serve_worker};

/// The processor contract itself, re-exported unchanged from [`sipx_audio::dsp`].
///
/// Writing a processor for a call means implementing [`FrameProcessor`], and a caller of this crate
/// should not have to name a second one to do it. These are that crate's own types, not a parallel
/// set: `docs/specs/custom-call-dsp.md` is where they are defined and this module reuses them
/// rather than minting anything beside them.
pub use sipx_audio::dsp::{
    Admitted, CapabilityError, DeadlineAction, DspCapability, DspFrame, DspObservation,
    DspResetCause, ExecutionPolicy, ExecutionProfile, FailureAction, FormatError, FrameAdmission,
    FrameProcessor, FrameSink, LengthPolicy, OverrunContainment, Parameter, ParameterDomain,
    ParameterError, ParameterSpec, ParameterValue, ProcessError, RateSupport, ResetBehavior,
    Scratch, StreamFormat,
};

/// The noise-reduction interface, re-exported unchanged from [`sipx_audio::dsp::noise`]
/// ([`docs/specs/call-dsp-noise-reduction.md`](../../../docs/specs/call-dsp-noise-reduction.md),
/// `M-66`).
///
/// Writing a noise reducer for a call means implementing [`NoiseReducer`] and declaring a
/// [`NoiseReduction`], and a caller of this crate should not have to name a second one to do it.
/// A reducer reaches a graph as an ordinary [`FrameProcessor`], so nothing here — not the plan, not
/// the graph, not the seam, not the session — can tell which implementation is installed. That is
/// the substitution the interface exists for; [`BuiltIn::SubbandSuppressor`] is the workspace's own
/// implementation of it, and its documentation states what it damages as well as what it removes.
pub use sipx_audio::dsp::noise::{
    ActivityHint, ActivityInput, DEFAULT_HOLD_POSITIONS, HintCause, HintChange, HintPolicy,
    HostRequirement, MAX_WARM_UP_POSITIONS, NOISE_REDUCTION_IDS, NoiseReducer, NoiseReduction,
    NoiseReductionError, SUBBAND_SUPPRESSOR, SubbandSuppressor,
};

pub(crate) use graph::{CallDsp, SlotRef};

/// One call direction's live DSP graph
/// ([`docs/specs/call-dsp-graph.md`](../../../docs/specs/call-dsp-graph.md) §3).
///
/// Returned by [`MediaSession::attach_dsp`](crate::MediaSession::attach_dsp). Holding it is what
/// keeps the graph attached: dropping it cancels every stage, and
/// [`detach`](Self::detach) is the same teardown with its barrier awaited.
pub struct DspGraph {
    slot: SlotRef,
}

/// What the graph is and what it is holding, never the audio it is holding (`M-107`, `M-68`).
///
/// A graph's buffers are the call's own audio — one of them is literally the frame in flight — and
/// a rendering that listed them would put raw call audio into whatever record named the graph, at a
/// length that is the frame's. This reports the same shape the accessors do, under one take of the
/// graph's lock rather than one per field, so a diagnostic never sees half of a transition either.
impl std::fmt::Debug for DspGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DspGraph")
            .field("slot", &self.slot)
            .finish()
    }
}

impl DspGraph {
    pub(crate) const fn new(slot: SlotRef) -> Self {
        Self { slot }
    }

    /// Which side of the call this graph transforms.
    #[must_use]
    pub fn direction(&self) -> AudioDirection {
        self.slot.direction()
    }

    /// The generation every frame currently sees, or 0 once the graph has been torn down.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.slot.generation()
    }

    /// Whether this graph may claim that over-budget work in it cannot stall RTP (§3.3).
    ///
    /// The conjunction of its stages' profiles: one
    /// [`TrustedCooperativeNative`](sipx_audio::dsp::ExecutionProfile::TrustedCooperativeNative)
    /// stage makes the whole chain uncontained, because it runs on the media worker and a callback
    /// that does not return stalls the media worker. Not configurable, and no conformance result or
    /// measurement changes it.
    #[must_use]
    pub fn contains_overrun(&self) -> bool {
        self.slot.contains_overrun()
    }

    /// The sum of the chain's stages' declared algorithmic latency, in positions.
    ///
    /// Summed from the declarations and never simulated. It does **not** include the lag a
    /// supervised stage's pipeline adds, which is [`Self::pipeline_frames`] and is counted in
    /// frames because that is the unit its deadline is stated in.
    #[must_use]
    pub fn latency_positions(&self) -> u64 {
        self.slot.latency_positions()
    }

    /// The processes this graph's supervised stages run in, in chain order (§7).
    ///
    /// Empty for a graph with no supervised stage, and empty once the graph has been torn down —
    /// a reaped worker is not a process any more, and reporting its old pid would name whatever the
    /// operating system reuses that number for next.
    ///
    /// This is how the process boundary is checked from outside rather than believed: an operator
    /// can inspect, limit or watch these processes, and a test can prove that the audio came from
    /// one of them. Bounding their memory and CPU is deployment configuration
    /// (`docs/specs/custom-call-dsp.md` §7.2), and these are the identifiers that configuration
    /// needs.
    #[must_use]
    pub fn worker_pids(&self) -> Vec<u32> {
        self.slot.worker_pids()
    }

    /// The lag this graph's supervised stages add, in frame durations (§7.1).
    ///
    /// The sum of their `deadline_frames`: a supervised stage's output lags its input by exactly
    /// the deadline it declared, which is what lets the media worker offer a frame and take a
    /// result without ever waiting for one.
    #[must_use]
    pub fn pipeline_frames(&self) -> u32 {
        self.slot.pipeline_frames()
    }

    /// Drain the typed transitions this direction has recorded (§5.3).
    ///
    /// The queue is bounded by the configured observation capacity and drops its oldest entries at
    /// capacity: a transition is a fact about the past, and the newest are the ones a caller can
    /// still act on.
    #[must_use]
    pub fn transitions(&self) -> Vec<GraphTransition> {
        self.slot.transitions()
    }

    /// What this graph is holding right now (§8).
    #[must_use]
    pub fn barrier(&self) -> GraphBarrier {
        self.slot.barrier()
    }

    /// What the runtime has observed about this direction's processors (`M-68`).
    ///
    /// Deadline misses, refusals, malformed results, lost workers, bypasses, resets and terminal
    /// failures, counted rather than queued: [`Self::transitions`] carries the detail and drops its
    /// oldest at capacity, and these cannot be dropped. Cumulative for the life of the call and not
    /// reset by a replacement or a teardown, so they still answer after the graph has ended.
    ///
    /// Nothing a *processor* observed about a frame is here — a
    /// [`DspObservation`] is the processor's own vocabulary, and
    /// an intentional glitch effect's `Saturated` is not a defect the runtime counted. See
    /// [`GraphCounters`].
    #[must_use]
    pub fn counters(&self) -> GraphCounters {
        self.slot.counters()
    }

    /// Validate a new chain whole and publish it as the next generation (§5.4).
    ///
    /// The outgoing generation is cancelled and its retained audio discarded rather than flushed —
    /// it belongs to an epoch that no longer exists — and the incoming one is prepared at position
    /// 0. Both happen under the same take a frame needs, so no frame sees a mixture.
    ///
    /// # Errors
    ///
    /// A [`GraphError`] naming the first thing §3.1 refuses. **A refused replacement leaves the
    /// graph in force untouched**: it is the caller's error, not the call's.
    ///
    /// The new chain is validated against the format and packetisation the **session** is running
    /// now, which is not necessarily the one this handle was created for: a graph outlives a
    /// [`MediaSession::reconfigure`](crate::MediaSession::reconfigure), and a handle that
    /// remembered its own sizing would go on preparing stages for a rate the call stopped carrying
    /// (`M-68`).
    pub fn replace(&self, plan: GraphPlan) -> Result<u64, GraphError> {
        self.slot.reinstall(plan)
    }

    /// Tear this graph down and wait for its barrier (§8).
    ///
    /// Cancels every stage, terminates and reaps every supervised worker, and resolves with the
    /// barrier it reached. The wait is an event and never a duration, so a caller that saw a clear
    /// barrier knows the graph holds nothing rather than believing it.
    pub async fn detach(&self) -> GraphBarrier {
        self.slot.cancel(TeardownCause::Detached);
        self.slot.settle().await
    }

    /// Wait until this graph has been torn down by anything at all, then report its barrier (§8).
    ///
    /// Resolves when the graph ends for any reason: a fail-closed failure, a detach, a stopped
    /// session, or a drop. This is how a caller observes a teardown it did not ask for without
    /// polling.
    pub async fn settled(&self) -> GraphBarrier {
        self.slot.settled().await
    }
}

impl Drop for DspGraph {
    /// Cancels every stage and terminates every supervised worker, without waiting.
    ///
    /// A drop cannot await, so it does not join: it closes each worker's request channel, which is
    /// what makes that worker's next receive return and its thread finish. [`Self::detach`] is the
    /// same teardown with the join awaited, and it is the one that can report a clear barrier.
    fn drop(&mut self) {
        self.slot.cancel(TeardownCause::Detached);
    }
}
