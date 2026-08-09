//! The bounds, the plan, the generation and the barrier of one call-local DSP graph
//! (`docs/specs/call-dsp-graph.md` §3 through §8).

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use sipx_audio::dsp::{
    CapabilityError, DspCapability, DspFrame, DspObservation, DspResetCause, ExecutionPolicy,
    ExecutionProfile, FailureAction, FormatError, FrameProcessor, FrameSink, LengthPolicy,
    MAX_FRAME_SAMPLES, MAX_LATENCY_POSITIONS, MAX_SCRATCH_SAMPLES, Parameter, ParameterError,
    Scratch, StreamFormat,
};

use super::builtin::BuiltIn;
use super::supervised::{Exchange, Supervised, WorkerProcess};
use crate::processing::{AudioDirection, DiscontinuityKind, Processing, hold};
use crate::session::Stop;

/// The most processors one chain may hold (`docs/specs/call-dsp-graph.md` §4).
///
/// The seam's own per-session attachment ceiling, reused: a graph is a chain on one direction of
/// one call, and eight is already headroom over the chains this epic serves.
pub const MAX_PROCESSORS: u32 = 8;

/// The deepest request or result channel a supervised stage may own (§4).
pub const MAX_WORKER_QUEUE: u32 = 64;

/// The seam's own queue domain, in the unit this module counts in.
///
/// Held against [`Processing`]'s figures at compile time rather than restated: the observation and
/// transition queue is the same kind of bounded per-call queue the seam's is, and two modules
/// disagreeing about how deep one may be is how a call ends up with two answers.
const DEFAULT_OBSERVATION_CAPACITY: u32 = 32;
const MAX_OBSERVATION_CAPACITY: u32 = 4_096;
const _: () = assert!(DEFAULT_OBSERVATION_CAPACITY as usize == Processing::DEFAULT_QUEUE_CAPACITY);
const _: () = assert!(MAX_OBSERVATION_CAPACITY as usize == Processing::MAX_QUEUE_CAPACITY);

// --------------------------------------------------------------------------- bounds ----

/// Every explicit, non-zero ceiling a call-local graph is sized by
/// (`docs/specs/call-dsp-graph.md` §4).
///
/// Each field is a ceiling a configuration is *refused* for exceeding rather than a hint, and none
/// of them may be zero: a zero bound is not "unbounded", it is a graph that can hold nothing, and
/// admitting it would make "explicit and non-zero" a sentence rather than a check. The upper ends
/// are the processor contract's and the seam's own, reused and not re-minted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GraphBounds {
    max_processors: u32,
    max_frame_samples: u32,
    scratch_samples: u32,
    retained_tail_positions: u32,
    observation_capacity: u32,
    worker_queue_capacity: u32,
}

impl Default for GraphBounds {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphBounds {
    /// The default configuration: the full chain, the contract's frame ceiling, 4,096 samples of
    /// scratch, a 4,096-position delay-line bound, the seam's default queue of 32 and an
    /// eight-deep worker channel.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_processors: MAX_PROCESSORS,
            max_frame_samples: MAX_FRAME_SAMPLES,
            scratch_samples: 4_096,
            retained_tail_positions: 4_096,
            observation_capacity: DEFAULT_OBSERVATION_CAPACITY,
            worker_queue_capacity: 8,
        }
    }

    /// How many processors the chain may hold. Domain `1..=`[`MAX_PROCESSORS`].
    #[must_use]
    pub const fn with_max_processors(mut self, processors: u32) -> Self {
        self.max_processors = processors;
        self
    }

    /// The largest interleaved frame a stage may declare. Domain `1..=65,536`.
    #[must_use]
    pub const fn with_max_frame_samples(mut self, samples: u32) -> Self {
        self.max_frame_samples = samples;
        self
    }

    /// The scratch region lent to each stage, in `i16`s. Domain `1..=65,536`.
    #[must_use]
    pub const fn with_scratch_samples(mut self, samples: u32) -> Self {
        self.scratch_samples = samples;
        self
    }

    /// The delay-line bound: the most latency or tail one stage may declare. Domain `1..=65,536`.
    #[must_use]
    pub const fn with_retained_tail_positions(mut self, positions: u32) -> Self {
        self.retained_tail_positions = positions;
        self
    }

    /// How many observations and transitions a graph queues before it drops the oldest.
    /// Domain `1..=4,096`.
    #[must_use]
    pub const fn with_observation_capacity(mut self, entries: u32) -> Self {
        self.observation_capacity = entries;
        self
    }

    /// How deep a supervised stage's request and result channels are.
    /// Domain `1..=`[`MAX_WORKER_QUEUE`].
    #[must_use]
    pub const fn with_worker_queue_capacity(mut self, frames: u32) -> Self {
        self.worker_queue_capacity = frames;
        self
    }

    /// The configured chain-length ceiling.
    #[must_use]
    pub const fn max_processors(&self) -> u32 {
        self.max_processors
    }

    /// The configured frame ceiling.
    #[must_use]
    pub const fn max_frame_samples(&self) -> u32 {
        self.max_frame_samples
    }

    /// The configured scratch region, in `i16`s.
    #[must_use]
    pub const fn scratch_samples(&self) -> u32 {
        self.scratch_samples
    }

    /// The configured delay-line bound, in positions.
    #[must_use]
    pub const fn retained_tail_positions(&self) -> u32 {
        self.retained_tail_positions
    }

    /// The configured observation and transition queue capacity.
    #[must_use]
    pub const fn observation_capacity(&self) -> u32 {
        self.observation_capacity
    }

    /// The configured depth of a supervised stage's channels.
    #[must_use]
    pub const fn worker_queue_capacity(&self) -> u32 {
        self.worker_queue_capacity
    }

    /// Whether every bound is inside its domain.
    ///
    /// # Errors
    ///
    /// [`GraphError::Bound`] naming the first field outside §4's domain for it.
    pub fn validate(&self) -> Result<(), GraphError> {
        bound("max_processors", self.max_processors, 1, MAX_PROCESSORS)?;
        bound(
            "max_frame_samples",
            self.max_frame_samples,
            1,
            MAX_FRAME_SAMPLES,
        )?;
        bound(
            "scratch_samples",
            self.scratch_samples,
            1,
            MAX_SCRATCH_SAMPLES,
        )?;
        bound(
            "retained_tail_positions",
            self.retained_tail_positions,
            1,
            MAX_LATENCY_POSITIONS,
        )?;
        bound(
            "observation_capacity",
            self.observation_capacity,
            1,
            MAX_OBSERVATION_CAPACITY,
        )?;
        bound(
            "worker_queue_capacity",
            self.worker_queue_capacity,
            1,
            MAX_WORKER_QUEUE,
        )
    }
}

fn bound(field: &'static str, value: u32, low: u32, high: u32) -> Result<(), GraphError> {
    if value < low || value > high {
        return Err(GraphError::Bound {
            field,
            value: u64::from(value),
        });
    }
    Ok(())
}

// --------------------------------------------------------------------------- errors ----

/// Why a plan was refused (`docs/specs/call-dsp-graph.md` §3, §4).
///
/// Every one of these refuses the **whole** plan. Nothing is prepared, nothing is allocated, no
/// worker is spawned, and the direction keeps running exactly as it was — with the previous graph
/// in force if there was one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GraphError {
    /// A bound is outside §4's domain for it.
    #[error("graph bound `{field}` is {value}, which is outside its domain")]
    Bound {
        /// The field, spelled as §4 spells it.
        field: &'static str,
        /// What was configured.
        value: u64,
    },
    /// The chain is longer than the configured ceiling.
    #[error("this chain holds more than the configured {limit} processors")]
    TooManyProcessors {
        /// The configured ceiling.
        limit: u32,
    },
    /// A stage's declaration is not internally admissible.
    #[error("the processor `{processor}` declares a capability that is not admissible")]
    Capability {
        /// The stage.
        processor: &'static str,
        /// What the processor contract's own `validate` said.
        source: CapabilityError,
    },
    /// A stage does not accept the call's audio format.
    #[error("the processor `{processor}` does not accept this call's audio format")]
    Format {
        /// The stage.
        processor: &'static str,
        /// What the processor contract's own `accepts_format` said.
        source: FormatError,
    },
    /// A stage declares a profile the door it arrived through does not admit (§3.2).
    ///
    /// An application-supplied processor may not declare itself proven: "proven" names evidence in
    /// this repository's gate, and an application cannot add to it. A supervised processor reaches
    /// a graph through the supervised door and not this one.
    #[error("the processor `{processor}` may not select the {profile:?} profile through this door")]
    ProfileNotAdmissible {
        /// The stage.
        processor: &'static str,
        /// What it declared.
        profile: ExecutionProfile,
    },
    /// A supervised stage declares a profile other than the supervised one.
    #[error("the supervised worker `{processor}` declares the {profile:?} profile")]
    NotSupervised {
        /// The stage.
        processor: &'static str,
        /// What it declared.
        profile: ExecutionProfile,
    },
    /// A stage may change a frame's position count, which a fixed packetisation cannot carry
    /// (§4.3).
    #[error("the processor `{processor}` may change a frame's length, which a live call cannot")]
    LengthNotPreserving {
        /// The stage.
        processor: &'static str,
    },
    /// A stage declares a larger frame than the configured ceiling.
    #[error(
        "the processor `{processor}` declares {declared} frame samples against a bound of {bound}"
    )]
    FrameExceedsBound {
        /// The stage.
        processor: &'static str,
        /// What it declared.
        declared: u32,
        /// What the bound admits.
        bound: u32,
    },
    /// A stage declares more scratch than the configured region.
    #[error(
        "the processor `{processor}` declares {declared} scratch samples against a bound of {bound}"
    )]
    ScratchExceedsBound {
        /// The stage.
        processor: &'static str,
        /// What it declared.
        declared: u32,
        /// What the bound admits.
        bound: u32,
    },
    /// A stage declares more latency or tail than the configured delay-line bound.
    ///
    /// Both are audio the graph holds, and neither may be unbounded, so the bound is held against
    /// the larger of the two.
    #[error(
        "the processor `{processor}` declares {declared} retained positions against a bound of {bound}"
    )]
    TailExceedsBound {
        /// The stage.
        processor: &'static str,
        /// What it declared.
        declared: u32,
        /// What the bound admits.
        bound: u32,
    },
    /// This direction already has a graph.
    ///
    /// Replacement is an operation with a generation, and an attach that quietly became one would
    /// produce a transition nobody asked for.
    #[error("this call already has a {direction} graph")]
    DirectionInUse {
        /// The direction already in use.
        direction: AudioDirection,
    },
    /// A stage refused the parameter set it was named with.
    ///
    /// Reached from [`GraphPlan::with_built_in`], where a workspace processor is configured before
    /// the plan is attached, and from [`DspGraph::configure`](super::DspGraph::configure), where a
    /// live stage's parameters are moved. Both refuse the **whole** set and leave the previous
    /// parameter state in force, which is `docs/specs/custom-call-dsp.md` §3.6's rule rather than a
    /// second one.
    #[error("the processor `{processor}` refused the parameters it was given")]
    Parameter {
        /// The stage.
        processor: &'static str,
        /// What the processor contract's own `configure` said.
        source: ParameterError,
    },
    /// A parameter update named a generation that is not the one now live (§10.2).
    ///
    /// A generation is what makes an update unambiguous: an application that composed a set
    /// against the chain it last saw must not have it land on the chain that replaced it, because
    /// stage 2 of one generation is a different processor from stage 2 of the next. The update is
    /// refused whole and the live graph is untouched.
    #[error("this update names generation {expected}, and generation {live} is live")]
    StaleGeneration {
        /// What the caller named.
        expected: u64,
        /// What is live. `0` means this direction has no graph at all.
        live: u64,
    },
    /// A parameter update named a stage index the live chain does not have (§10.2).
    #[error("this chain has {processors} stages, so there is no stage {index}")]
    UnknownProcessor {
        /// What the caller named.
        index: u32,
        /// How many stages the live chain holds.
        processors: u32,
    },
    /// A parameter update named a stage whose parameters cannot be moved while it runs (§10.2).
    ///
    /// A supervised stage runs behind §7.4's framed protocol, and that protocol carries `Hello`,
    /// `Frame` and `Result` and nothing else. There is no message for a parameter set, so an
    /// update to one is refused rather than accepted and dropped: an application that needs a
    /// supervised worker configured differently replaces the graph, which is how a new worker
    /// process is asked for (§7.2).
    #[error("the supervised worker `{processor}` has no wire for a parameter set")]
    NotConfigurable {
        /// The stage.
        processor: &'static str,
    },
    /// The session has stopped, so a graph attached to it could never see a frame.
    #[error("this session has stopped")]
    SessionStopped,
    /// The handle's graph has already been torn down.
    #[error("this graph has been torn down")]
    Detached,
    /// A supervised stage's worker program could not be spawned (§7.4).
    ///
    /// Carries the operating system's own classification rather than its message, because this
    /// error is `Copy` and comparable: a caller that wants the message has the program name and can
    /// say it better than a formatted `io::Error` would.
    #[error("the supervised worker `{processor}` could not be spawned: {error:?}")]
    WorkerSpawn {
        /// The stage.
        processor: &'static str,
        /// What the operating system said.
        error: std::io::ErrorKind,
    },
}

// ----------------------------------------------------------------------- transitions ----

/// Why a stage stopped contributing (`docs/specs/call-dsp-graph.md` §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BypassCause {
    /// The application asked for it.
    Requested,
    /// The stage refused its frames until it had failed under the configured miss budget.
    Refused,
    /// No result was present by the configured deadline.
    DeadlineMissed,
    /// A supervised worker answered with a position count other than the frame's, or an inline
    /// processor produced one.
    MalformedResult,
    /// A supervised worker failed terminally, panicked, or its channel closed.
    WorkerLost,
}

/// Why a graph was torn down (`docs/specs/call-dsp-graph.md` §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TeardownCause {
    /// The application asked for it.
    Requested,
    /// The handle was detached or dropped.
    Detached,
    /// The session stopped, was shut down, or was dropped.
    SessionStopped,
    /// A renegotiation changed the audio format or the packetisation this graph was prepared for
    /// (`M-68`).
    ///
    /// A graph otherwise survives a `MediaSession::reconfigure`, because it belongs to the call
    /// rather than to a worker generation. What it cannot survive is a change to the two things it
    /// was *validated* against: every stage was prepared for one
    /// [`StreamFormat`](sipx_audio::dsp::StreamFormat), and
    /// [`docs/specs/custom-call-dsp.md`](../../../docs/specs/custom-call-dsp.md) §8.3 makes a rate
    /// change a `prepare` and never a frame — so carrying the chain over would hand every stage
    /// audio at a rate it never agreed to, under a declaration saying otherwise. Every buffer was
    /// also sized from the old packetisation, and §4 forbids growing one.
    ///
    /// The graph is torn down rather than re-prepared, and nothing is re-attached behind the
    /// application's back. A supervised stage cannot be re-prepared at all — its worker's `Hello`
    /// named the old format and §7.2 forbids a silent respawn — and an inline stage's audio state
    /// belongs to an epoch at the old rate. Attaching a graph for the new format is the
    /// application's call to make, exactly as replacing one is.
    FormatChanged,
    /// A stage configured [`FailureAction::TerminateClosed`] failed.
    ///
    /// The audio it was protecting does not flow: the direction is silenced rather than carrying
    /// audio that was supposed to have been processed.
    FailedClosed {
        /// The stage that failed.
        processor: &'static str,
    },
}

/// One typed change to what a frame will see (`docs/specs/call-dsp-graph.md` §5.3).
///
/// Every variant names the generation it belongs to and the position at which it took effect — the
/// first position of the first frame it applies to, in the graph's current epoch. A change is
/// always applied *between* frames, so no frame ever saw half of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GraphTransition {
    /// A validated graph became the one every subsequent frame sees.
    Activated {
        /// The new generation, counting from 1.
        generation: u64,
        /// Where in the epoch it took effect. An activation opens an epoch, so this is 0.
        at_position: u64,
        /// How many stages it holds.
        processors: u32,
    },
    /// A validated graph replaced an earlier generation, whole.
    Replaced {
        /// The new generation.
        generation: u64,
        /// The generation it replaced.
        previous: u64,
        /// Where in the epoch it took effect. A replacement opens a new epoch, so this is 0.
        at_position: u64,
        /// How many stages the new generation holds.
        processors: u32,
    },
    /// One stage's parameters were moved, whole, at a position boundary (§5.3, `M-67`).
    ///
    /// The set was validated against the stage's declared schema before anything was assigned, so
    /// this is reported only for a set that applied entire. A refused set produces no transition at
    /// all, because nothing about what a frame will see changed.
    ///
    /// It carries the stage and the boundary rather than the values. The values are the
    /// application's own — it wrote them — and putting them here would make a bounded queue's
    /// entry size depend on how many parameters a processor declares.
    Configured {
        /// The generation the stage belongs to.
        generation: u64,
        /// The first position of the first frame the new values apply to.
        at_position: u64,
        /// The stage.
        processor: &'static str,
    },
    /// One stage stopped contributing; the frames that follow it carry a discontinuity.
    Bypassed {
        /// The generation the stage belongs to.
        generation: u64,
        /// Where in the epoch it stopped contributing.
        at_position: u64,
        /// The stage.
        processor: &'static str,
        /// Why.
        cause: BypassCause,
    },
    /// The graph was torn down: no processor of this generation will see another frame.
    TornDown {
        /// The generation that ended.
        generation: u64,
        /// Where in the epoch it ended.
        at_position: u64,
        /// Why.
        cause: TeardownCause,
    },
}

/// One parameter update that applied, and where (`docs/specs/call-dsp-graph.md` §10.2, `M-67`).
///
/// The **terminal outcome** of [`DspGraph::configure`](super::DspGraph::configure), and there is
/// exactly one per update: either this, or a [`GraphError`] and nothing changed. It is returned
/// rather than only journalled because the journal is a bounded queue whose superseded parameter
/// entries coalesce (§5.3), and an outcome a caller is waiting on may not be something a later
/// update can absorb.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParameterUpdate {
    generation: u64,
    at_position: u64,
    processor: &'static str,
}

impl ParameterUpdate {
    /// The generation the update applied to — the one the caller named, checked and not assumed.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// The first position of the first frame the new values apply to, in the graph's own epoch.
    ///
    /// A position and never a clock reading: the same update against the same call applies at the
    /// same boundary on every host.
    #[must_use]
    pub const fn at_position(&self) -> u64 {
        self.at_position
    }

    /// The stage the values were assigned to.
    #[must_use]
    pub const fn processor(&self) -> &'static str {
        self.processor
    }
}

/// What a graph is holding (`docs/specs/call-dsp-graph.md` §8).
///
/// The barrier teardown waits for. It is **clear** when all four are zero, and a caller that saw a
/// clear barrier knows the graph holds nothing rather than believing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct GraphBarrier {
    workers: u32,
    frames_in_flight: u32,
    retained_positions: u64,
    processors: u32,
}

impl GraphBarrier {
    /// Whether the graph holds nothing at all.
    #[must_use]
    pub const fn is_clear(&self) -> bool {
        self.workers == 0
            && self.frames_in_flight == 0
            && self.retained_positions == 0
            && self.processors == 0
    }

    /// Supervised workers still running. Zero means every one was terminated and reaped.
    #[must_use]
    pub const fn workers(&self) -> u32 {
        self.workers
    }

    /// Frames offered to a request channel whose result has not been accounted for.
    #[must_use]
    pub const fn frames_in_flight(&self) -> u32 {
        self.frames_in_flight
    }

    /// The sum of `retained()` over the stages, read back rather than assumed.
    #[must_use]
    pub const fn retained_positions(&self) -> u64 {
        self.retained_positions
    }

    /// Stages still installed. Zero means the generation is gone.
    #[must_use]
    pub const fn processors(&self) -> u32 {
        self.processors
    }

    fn add(self, other: Self) -> Self {
        Self {
            workers: self.workers.saturating_add(other.workers),
            frames_in_flight: self.frames_in_flight.saturating_add(other.frames_in_flight),
            retained_positions: self
                .retained_positions
                .saturating_add(other.retained_positions),
            processors: self.processors.saturating_add(other.processors),
        }
    }
}

// -------------------------------------------------------------------------- counters ----

/// What the runtime observed about one direction's processors (`M-68`).
///
/// Every figure here is a fact the **runtime** observed about a processor. None of them is a fact a
/// processor observed about a frame — those are
/// [`DspObservation`](sipx_audio::dsp::DspObservation)s, they are the processor's own vocabulary,
/// and they never arrive here. That split is
/// [`docs/specs/custom-call-dsp.md`](../../../docs/specs/custom-call-dsp.md) §6's and it is
/// load-bearing rather than tidy: an effect that is *supposed* to sound broken reports a
/// `Saturated` or a `PassedThrough` about the audio it made, and a processor that is failing its
/// budget increments a counter here. No door of this stack may present the second as the first, so
/// there is no counter here an intentional glitch effect can move.
///
/// **They are cumulative for the life of the call**, across every generation of the graph. A
/// replacement does not reset them and neither does a teardown, which is the point: after a
/// fail-closed teardown the graph is gone and these are what is left to say why.
///
/// Counters are what survives a bounded queue. [`GraphTransition`]s are the detail — which stage,
/// which position, which generation — and the transition queue drops its oldest at capacity
/// ([`GraphBounds::observation_capacity`]). A count cannot be dropped, so a caller that only ever
/// reads these still knows what happened, and [`Self::transitions_dropped`] says how much detail
/// it did not get.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct GraphCounters {
    frames: u64,
    deadline_misses: u64,
    refusals: u64,
    malformed_results: u64,
    workers_lost: u64,
    bypasses: u64,
    resets: u64,
    teardowns: u64,
    terminal_failures: u64,
    frames_passed_through: u64,
    transitions_dropped: u64,
    transitions_coalesced: u64,
}

impl GraphCounters {
    /// Frames this direction's graph was offered, whether or not a stage transformed one.
    #[must_use]
    pub const fn frames(&self) -> u64 {
        self.frames
    }

    /// Stage-frames with no result by the configured deadline
    /// (`docs/specs/call-dsp-graph.md` §7.2).
    ///
    /// Counted per stage and per frame, so a chain of two late stages counts two on one frame.
    /// A supervised stage's pipeline filling counts here as well: "no result by the deadline" is
    /// the same fact whatever caused it (§7.1).
    #[must_use]
    pub const fn deadline_misses(&self) -> u64 {
        self.deadline_misses
    }

    /// Stage-frames an inline processor refused with a typed [`ProcessError`].
    ///
    /// [`ProcessError`]: sipx_audio::dsp::ProcessError
    #[must_use]
    pub const fn refusals(&self) -> u64 {
        self.refusals
    }

    /// Stage-frames whose result carried a position count other than the frame's (§7.2).
    ///
    /// Well formed and wrong — on the wire for a supervised worker, and in the sink for an inline
    /// processor. Distinct from [`Self::refusals`] because a processor that says no and a
    /// processor that answers the wrong question are different defects.
    #[must_use]
    pub const fn malformed_results(&self) -> u64 {
        self.malformed_results
    }

    /// Stage-frames lost to a worker that exited, crashed, was killed or failed terminally (§7.2).
    #[must_use]
    pub const fn workers_lost(&self) -> u64 {
        self.workers_lost
    }

    /// Stages that spent their miss budget and stopped contributing under
    /// [`FailureAction::BypassOpen`] (§6.1).
    ///
    /// One per stage per generation: a bypassed stage stays bypassed until the graph is replaced.
    #[must_use]
    pub const fn bypasses(&self) -> u64 {
        self.bypasses
    }

    /// Stage resets the graph asked for because the session's own timeline broke (§6.2).
    ///
    /// Counted per stage, as the resets are. A processor's internal reset on a flagged frame is
    /// the processor's own and is not counted here; this is the graph re-anchoring a whole chain.
    #[must_use]
    pub const fn resets(&self) -> u64 {
        self.resets
    }

    /// Generations torn down, for any cause (§5.3).
    #[must_use]
    pub const fn teardowns(&self) -> u64 {
        self.teardowns
    }

    /// Teardowns caused by a stage configured [`FailureAction::TerminateClosed`] failing (§6.1).
    ///
    /// A subset of [`Self::teardowns`], separated because it is the only one the call did not ask
    /// for: the audio this direction was carrying stopped because a processor whose absence is a
    /// policy breach was absent.
    #[must_use]
    pub const fn terminal_failures(&self) -> u64 {
        self.terminal_failures
    }

    /// Frames handed on untouched because they were larger than the graph was sized for (§4).
    ///
    /// The graph's buffers are sized at validation from the session's packetisation and never
    /// grown, so a longer frame passes through and the next one carries a break. A non-zero figure
    /// here means this direction's audio is **not** being processed and nothing is failing to say
    /// so — the reason this is a counter and not only a transition.
    #[must_use]
    pub const fn frames_passed_through(&self) -> u64 {
        self.frames_passed_through
    }

    /// Transitions the bounded queue dropped before a caller drained it (§5.3).
    ///
    /// The queue drops its **oldest** at capacity. This is how much detail was lost; the counters
    /// beside it are what was not.
    #[must_use]
    pub const fn transitions_dropped(&self) -> u64 {
        self.transitions_dropped
    }

    /// Superseded parameter states the queue coalesced rather than dropped (§5.3, `M-67`).
    ///
    /// Read beside [`Self::transitions_dropped`] and not added to it: a coalesced entry is one an
    /// application replaced itself, so this rising is normal for a caller moving parameters faster
    /// than it drains, while the figure beside it rising is detail nobody can recover.
    #[must_use]
    pub const fn transitions_coalesced(&self) -> u64 {
        self.transitions_coalesced
    }

    /// Every stage-frame that missed, whatever the cause (§6.1's miss).
    ///
    /// The sum of [`Self::deadline_misses`], [`Self::refusals`], [`Self::malformed_results`] and
    /// [`Self::workers_lost`] — the four things §6.1 counts against a stage's consecutive-miss
    /// budget, and nothing else.
    #[must_use]
    pub const fn misses(&self) -> u64 {
        self.deadline_misses
            .saturating_add(self.refusals)
            .saturating_add(self.malformed_results)
            .saturating_add(self.workers_lost)
    }

    /// Count one miss under the cause the transition would name.
    fn miss(&mut self, cause: BypassCause) {
        let slot = match cause {
            BypassCause::DeadlineMissed => &mut self.deadline_misses,
            BypassCause::Refused => &mut self.refusals,
            BypassCause::MalformedResult => &mut self.malformed_results,
            // `Requested` is not a miss and never reaches here; counting it as a lost worker would
            // be wrong, and a build that met a cause a later revision adds must not lose it.
            BypassCause::WorkerLost | _ => &mut self.workers_lost,
        };
        *slot = slot.saturating_add(1);
    }
}

// ------------------------------------------------------------------------------ plan ----

/// One stage of a plan, before it has been validated or prepared.
enum Planned {
    Inline {
        processor: Box<dyn FrameProcessor + Send>,
        /// Whether this stage came through the workspace registry (§3.2).
        ///
        /// Provenance is per stage rather than per plan, deliberately: a chain that mixes a
        /// built-in with an application-supplied processor must not let the built-in's provenance
        /// admit the application's `ProvenInline` declaration beside it.
        workspace: bool,
    },
    Supervised(Box<WorkerProcess>),
}

impl Planned {
    fn capability(&self) -> DspCapability {
        match self {
            Self::Inline { processor, .. } => processor.capability(),
            Self::Supervised(worker) => worker.capability(),
        }
    }
}

/// An ordered chain of processors for one direction, and the bounds it is sized by
/// (`docs/specs/call-dsp-graph.md` §3).
///
/// A plan is data. Nothing in it is prepared, allocated or spawned until it is attached, or used to
/// replace a live graph — and at that point it is validated **whole**: the first refusal refuses all
/// of it, and the direction keeps running exactly as it was (§3.1).
pub struct GraphPlan {
    direction: AudioDirection,
    bounds: GraphBounds,
    stages: Vec<Planned>,
    /// Whether this plan reached the crate-internal door and may carry a proven stage (§3.2).
    workspace: bool,
}

impl std::fmt::Debug for GraphPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphPlan")
            .field("direction", &self.direction)
            .field("bounds", &self.bounds)
            .field(
                "stages",
                &self
                    .stages
                    .iter()
                    .map(|stage| stage.capability().id())
                    .collect::<Vec<_>>(),
            )
            .field("workspace", &self.workspace)
            .finish()
    }
}

impl GraphPlan {
    /// An empty chain for one direction.
    #[must_use]
    pub const fn new(direction: AudioDirection, bounds: GraphBounds) -> Self {
        Self {
            direction,
            bounds,
            stages: Vec::new(),
            workspace: false,
        }
    }

    /// Append one application-supplied processor, to run inline on the media worker.
    ///
    /// Order is the order of these calls: index 0 first, and stage *k*'s output is stage *k+1*'s
    /// input. The processor's declared execution profile decides what attaching it may claim, and
    /// this door admits only `TrustedCooperativeNative`: `ProvenInline` names evidence in this
    /// repository's gate that an application cannot add to, and `SupervisedIsolated` reaches a
    /// graph through [`Self::with_supervised`] because it is not run on the media worker at all
    /// (§3.2).
    ///
    /// # What this door promises, in full, because the honest description is the feature
    ///
    /// **It claims nothing about containment.** The processor handed here is called directly on the
    /// media worker, by explicit selection, and sipx cannot preempt it, cannot cancel it, cannot
    /// reap it and cannot bound its memory. A `process` call that does not return stalls the media
    /// worker and therefore **stalls RTP for that call**, and no configured
    /// [`ExecutionPolicy`](sipx_audio::dsp::ExecutionPolicy) changes that: the deadline, the miss
    /// budget, the failure action and the teardown barrier are every one of them code that runs
    /// *after* the callback returns. A processor here may copy audio it was lent, spawn work this
    /// stack does not own, and outlive the call. One such stage anywhere in a chain makes
    /// [`DspGraph::contains_overrun`](super::DspGraph::contains_overrun) false for the whole graph
    /// (§3.3), and no conformance result, counter or measurement sets it true.
    ///
    /// [`Self::with_supervised`] is the door for an application that needs the stack to *contain* a
    /// stall rather than to report one. It costs a process and a frame of pipeline latency, and it
    /// is the only application-reachable profile that may make the claim.
    #[must_use]
    pub fn with_processor(mut self, processor: Box<dyn FrameProcessor + Send>) -> Self {
        self.stages.push(Planned::Inline {
            processor,
            workspace: false,
        });
        self
    }

    /// Append one of this workspace's own processors, configured with a parameter set (§3.2).
    ///
    /// This is the registry door, and the only one that admits
    /// [`ExecutionProfile::ProvenInline`]: what it grants is provenance, not access. The same
    /// processor constructed by hand and handed to [`Self::with_processor`] is still refused
    /// `ProfileNotAdmissible`, because "proven" names evidence in this repository's gate and the
    /// door is what carries that claim rather than the type.
    ///
    /// The parameter set is the processor contract's own vocabulary and is applied here, before the
    /// plan is attached, so a stage is configured by the time any buffer is sized from it.
    ///
    /// # Errors
    ///
    /// [`GraphError::Capability`] for a shape the processor does not admit — a
    /// [`BuiltIn::Stutter`] line past the effect's own maximum — and [`GraphError::Parameter`] for
    /// a set outside the declared schema. Both refuse at the call that wrote them; nothing is
    /// attached, and there is no plan to leave half-built.
    pub fn with_built_in(
        mut self,
        built_in: BuiltIn,
        parameters: &[Parameter],
    ) -> Result<Self, GraphError> {
        let mut processor = built_in
            .processor()
            .map_err(|source| GraphError::Capability {
                processor: built_in.id(),
                source,
            })?;
        processor
            .configure(parameters)
            .map_err(|source| GraphError::Parameter {
                processor: built_in.id(),
                source,
            })?;
        self.stages.push(Planned::Inline {
            processor,
            workspace: true,
        });
        Ok(self)
    }

    /// Append one supervised worker, to run in a process of its own behind bounded channels.
    ///
    /// Its declared capability must name the supervised profile. This is the door an application
    /// selects when it requires the stack to contain a stall rather than to report one, and what it
    /// names is a **program**: the worker runs in an operating-system process this runtime spawns,
    /// reached over `docs/specs/call-dsp-graph.md` §7.4's protocol, so a worker that will not stop
    /// can be killed and a worker's memory and CPU are bounded by whatever the operating system is
    /// configured to bound (`M-102`).
    #[must_use]
    pub fn with_supervised(mut self, worker: WorkerProcess) -> Self {
        self.stages.push(Planned::Supervised(Box::new(worker)));
        self
    }

    /// Which direction this chain is for.
    #[must_use]
    pub const fn direction(&self) -> AudioDirection {
        self.direction
    }

    /// The bounds this chain is sized by.
    #[must_use]
    pub const fn bounds(&self) -> GraphBounds {
        self.bounds
    }

    /// How many stages the chain holds.
    #[must_use]
    pub fn processors(&self) -> u32 {
        u32::try_from(self.stages.len()).unwrap_or(u32::MAX)
    }

    /// The crate-internal door of §3.2, which admits the proven-inline profile.
    ///
    /// Reached only from the workspace processor registry, which is empty until `M-65` ships
    /// processors to put in it — so today no caller outside this crate's own tests can reach it,
    /// which is the correct state of a workspace that has proven nothing yet.
    #[cfg(test)]
    pub(crate) const fn with_workspace_provenance(mut self) -> Self {
        self.workspace = true;
        self
    }
}

// ----------------------------------------------------------------------------- stage ----

/// One validated, prepared stage of a live graph.
struct Stage {
    capability: DspCapability,
    policy: ExecutionPolicy,
    kind: Running,
    misses: u32,
    bypassed: bool,
}

/// Which processor this is and how it is faring, rather than everything it declared (`M-68`).
///
/// The whole capability is available to a caller from its own declaration and re-rendering it here
/// makes a stage's record longer than anything reading it wants; what a diagnostic needs is the
/// identity, the profile — because that is what decides what this stage may claim — and the miss
/// budget it has spent.
impl std::fmt::Debug for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stage")
            .field("processor", &self.capability.id())
            .field("profile", &self.policy.profile())
            .field("misses", &self.misses)
            .field("bypassed", &self.bypassed)
            .field("kind", &self.kind)
            .finish()
    }
}

enum Running {
    Inline(Box<dyn FrameProcessor + Send>),
    Supervised(Box<Supervised>),
}

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // A processor is a boxed trait object with no `Debug` bound — the contract does not ask
            // for one, and requiring it here would be this attachment adding to it.
            Self::Inline(_) => f.write_str("Inline"),
            Self::Supervised(supervised) => f.debug_tuple("Supervised").field(supervised).finish(),
        }
    }
}

impl Stage {
    fn retained(&self) -> u64 {
        match &self.kind {
            Running::Inline(processor) => u64::from(processor.retained()),
            // A supervised stage's own sample memory lives in its worker, which this side cannot
            // read. What it can read is what it lent that worker, which is `frames_in_flight`.
            Running::Supervised(_) => 0,
        }
    }

    fn frames_in_flight(&self) -> u32 {
        match &self.kind {
            Running::Inline(_) => 0,
            Running::Supervised(supervised) => supervised.frames_in_flight(),
        }
    }

    fn workers(&self) -> u32 {
        match &self.kind {
            Running::Inline(_) => 0,
            Running::Supervised(supervised) => u32::from(supervised.is_live()),
        }
    }

    /// The operating-system process this stage's worker runs in, if it has one (§7).
    fn worker_pid(&self) -> Option<u32> {
        match &self.kind {
            Running::Inline(_) => None,
            Running::Supervised(supervised) => supervised.is_live().then(|| supervised.pid()),
        }
    }

    fn reset(&mut self, cause: DspResetCause) {
        if let Running::Inline(processor) = &mut self.kind {
            processor.reset(cause);
        }
    }

    /// Terminal and idempotent, and the processor's half of the barrier
    /// (`docs/specs/custom-call-dsp.md` §8.4).
    fn cancel(&mut self) {
        match &mut self.kind {
            Running::Inline(processor) => processor.cancel(),
            Running::Supervised(supervised) => supervised.terminate(),
        }
    }

    async fn reap(&mut self) {
        if let Running::Supervised(supervised) = &mut self.kind {
            supervised.reap().await;
        }
    }
}

// --------------------------------------------------------------------------- buffers ----

/// The graph's whole workspace, allocated at validation and never on the live path.
#[derive(Default)]
struct Buffers {
    front: Vec<i16>,
    back: Vec<i16>,
    scratch: Vec<i16>,
    observations: Vec<DspObservation>,
    observation_capacity: u32,
}

/// What the workspace is, never what is in it (`M-107`, `M-68`).
///
/// `front`, `back` and `scratch` are the call's own audio — `front` is literally the frame the
/// graph is carrying — so a derived rendering of this struct puts up to three frames of raw call
/// audio into whatever record names it, at a *length that is the frame's*. Reported as shape for
/// exactly the reason [`DspFrame`] and [`FrameSink`] are. The observations are counted rather than
/// listed for the second half of that rule: their number is a caller's configured bound.
impl std::fmt::Debug for Buffers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buffers")
            .field("carrying", &self.front.len())
            .field("staging", &self.back.len())
            .field("frame_capacity", &self.front.capacity())
            .field("scratch", &self.scratch.len())
            .field("observations", &self.observations.len())
            .field("observation_capacity", &self.observation_capacity)
            .finish()
    }
}

// ------------------------------------------------------------------------ generation ----

/// One live generation of one direction's chain.
struct Live {
    generation: u64,
    format: StreamFormat,
    stages: Vec<Stage>,
    buffers: Buffers,
    position: u64,
    /// The frame this graph's buffers were sized for, from the session's packetisation.
    ///
    /// A longer frame is passed through rather than grown into: a bound that grows under pressure
    /// is not a bound, and this is what makes "the live path allocates nothing" exact rather than
    /// nearly true.
    frame_samples: usize,
    /// A break the session declared that the next frame must carry.
    pending: Option<DiscontinuityKind>,
    contains_overrun: bool,
    latency_positions: u64,
    pipeline_frames: u32,
}

/// What this generation is, without restating what the slot around it already says (`M-68`).
///
/// `format` and `frame_samples` are omitted because they are the slot's `sizing`, which every
/// generation of that slot shares; a record that said each of them twice would be longer without
/// being more informative.
#[expect(
    clippy::missing_fields_in_debug,
    reason = "the two omitted fields are the slot's `sizing`, rendered once there"
)]
impl std::fmt::Debug for Live {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Live")
            .field("generation", &self.generation)
            .field("stages", &self.stages)
            .field("buffers", &self.buffers)
            .field("position", &self.position)
            .field("pending", &self.pending)
            .field("contains_overrun", &self.contains_overrun)
            .field("latency_positions", &self.latency_positions)
            .field("pipeline_frames", &self.pipeline_frames)
            .finish()
    }
}

impl Live {
    fn barrier(&self) -> GraphBarrier {
        GraphBarrier {
            workers: self.stages.iter().map(Stage::workers).sum(),
            frames_in_flight: self.stages.iter().map(Stage::frames_in_flight).sum(),
            retained_positions: self.stages.iter().map(Stage::retained).sum(),
            processors: u32::try_from(self.stages.len()).unwrap_or(u32::MAX),
        }
    }

    /// The processes this generation's supervised stages run in, in chain order (§7).
    fn worker_pids(&self) -> Vec<u32> {
        self.stages.iter().filter_map(Stage::worker_pid).collect()
    }

    /// Run one frame through the whole chain, in place.
    ///
    /// Returns the stage that failed closed, if one did. Never awaits, never blocks and never
    /// allocates (§6.3).
    fn run(
        &mut self,
        direction: AudioDirection,
        samples: &mut Vec<i16>,
        counters: &mut GraphCounters,
        journal: &mut Journal,
    ) -> Frame {
        let positions = samples.len() as u64;
        if positions == 0 {
            return Frame::default();
        }
        counters.frames = counters.frames.saturating_add(1);
        let generation = self.generation;
        let format = self.format;
        let position = self.position;

        if samples.len() > self.frame_samples {
            // Larger than the graph was sized for at validation. It passes through untouched and
            // the next frame carries the break, because growing a buffer here would make §4's frame
            // bound a suggestion. A session's packetisation is fixed, so this is a broken producer
            // rather than a condition to design around.
            counters.frames_passed_through = counters.frames_passed_through.saturating_add(1);
            self.pending = Some(merge(self.pending, DiscontinuityKind::Loss));
            self.position = self.position.saturating_add(positions);
            return Frame::default();
        }

        let mut buffers = std::mem::take(&mut self.buffers);
        // Copied in and copied out rather than swapped with the caller's buffer: the graph keeps
        // both of its own for its whole life, so neither can end up shorter than it was sized for
        // and no frame can make it allocate.
        buffers.front.clear();
        buffers.front.extend_from_slice(samples);

        let mut report = Frame::default();
        // §6.2: once the signal has changed shape, *every* stage after that point is told. The flag
        // is sticky for the rest of the frame rather than being consumed by the first stage that
        // sees it, because each of them is filtering a signal that is no longer the one it was.
        let mut downstream = self.pending.take();

        for stage in &mut self.stages {
            if stage.bypassed {
                downstream = Some(merge(downstream, DiscontinuityKind::Loss));
                continue;
            }
            match run_stage(stage, direction, format, position, downstream, &mut buffers) {
                StageOutcome::Produced => {
                    stage.misses = 0;
                    std::mem::swap(&mut buffers.front, &mut buffers.back);
                }
                StageOutcome::Missed(cause) => {
                    counters.miss(cause);
                    downstream = Some(merge(downstream, break_for(cause)));
                    stage.misses = stage.misses.saturating_add(1);
                    if stage.misses < stage.policy.max_consecutive_misses() {
                        continue;
                    }
                    if matches!(stage.policy.on_failure(), FailureAction::TerminateClosed) {
                        report.failed_closed = Some(stage.capability.id());
                        break;
                    }
                    // `BypassOpen`, and the safe reading of any action a later revision of the
                    // contract adds: an action this build does not recognise must not silence a
                    // call on a guess.
                    stage.bypassed = true;
                    counters.bypasses = counters.bypasses.saturating_add(1);
                    // Journalled here rather than carried out on the frame's report, because more
                    // than one stage of a chain can spend its budget on the same frame and a
                    // report with room for one of them would silently keep the last (`M-68`).
                    journal.push(GraphTransition::Bypassed {
                        generation,
                        at_position: position,
                        processor: stage.capability.id(),
                        cause,
                    });
                }
            }
        }

        // The output has exactly the input's position count — every stage is `Preserving` (§4.3) —
        // so refilling the caller's buffer cannot grow it either.
        samples.clear();
        samples.extend_from_slice(&buffers.front);
        self.buffers = buffers;
        self.position = self.position.saturating_add(positions);
        // A break no stage could be told about — because there are no stages — stays owed to the
        // next frame rather than being lost.
        self.pending = if self.stages.is_empty() {
            downstream
        } else {
            None
        };
        report
    }

    /// Discard every stage's sample memory and reopen the epoch at position 0 (§6.2).
    fn realign(&mut self, kind: DiscontinuityKind, counters: &mut GraphCounters) {
        for stage in &mut self.stages {
            stage.reset(DspResetCause::Discontinuity { kind });
            // §6.1: a run of misses is a processor that does not fit its budget on this machine,
            // and a re-anchored epoch is a different question from the one that run was asking.
            // A bypassed stage stays bypassed: its budget was already spent, and a break in the
            // timeline is not evidence that it will now keep up.
            stage.misses = 0;
            counters.resets = counters.resets.saturating_add(1);
        }
        self.position = 0;
        self.pending = None;
    }

    fn cancel(&mut self) {
        for stage in &mut self.stages {
            stage.cancel();
        }
    }

    async fn reap(&mut self) {
        for stage in &mut self.stages {
            stage.reap().await;
        }
        self.stages.clear();
    }
}

/// What running one frame produced besides audio.
///
/// One field, because a bypass is journalled where it happens: only a fail-closed teardown has to
/// come back out, since retiring a generation needs the whole slot rather than its journal.
#[derive(Debug, Default, Clone, Copy)]
struct Frame {
    failed_closed: Option<&'static str>,
}

/// What one stage did with one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StageOutcome {
    Produced,
    Missed(BypassCause),
}

/// Which break a miss hands downstream (`docs/specs/call-dsp-graph.md` §6.2).
const fn break_for(cause: BypassCause) -> DiscontinuityKind {
    match cause {
        // A bounded result channel that did not yield this frame's result is a bounded queue that
        // dropped it, which is what the seam calls `Overflow`.
        BypassCause::DeadlineMissed | BypassCause::MalformedResult => DiscontinuityKind::Overflow,
        // Everything else is processed audio that never became a frame.
        _ => DiscontinuityKind::Loss,
    }
}

/// Coalesce two breaks into the more disruptive one, in the seam's own order.
fn merge(pending: Option<DiscontinuityKind>, next: DiscontinuityKind) -> DiscontinuityKind {
    match pending {
        Some(DiscontinuityKind::Realign) => DiscontinuityKind::Realign,
        Some(DiscontinuityKind::Overflow) => DiscontinuityKind::Overflow,
        _ => next,
    }
}

/// Run one stage over `buffers.front`, writing into `buffers.back`.
///
/// The discontinuity is set on the frame and the processor is **not** reset from outside: the
/// contract puts the reset inside the processor, before the flagged frame's own samples are
/// consumed (`docs/specs/custom-call-dsp.md` §8.1), and resetting it here as well would discard
/// twice what the processor is required to discard once.
fn run_stage(
    stage: &mut Stage,
    direction: AudioDirection,
    format: StreamFormat,
    position: u64,
    downstream: Option<DiscontinuityKind>,
    buffers: &mut Buffers,
) -> StageOutcome {
    match &mut stage.kind {
        Running::Inline(processor) => {
            let positions = buffers.front.len();
            let mut frame = DspFrame::new(direction, format, position, &buffers.front);
            if let Some(kind) = downstream {
                frame = frame.with_discontinuity(kind);
            }
            buffers.back.clear();
            buffers.back.resize(positions, 0);
            buffers.observations.clear();
            let capacity = buffers.observation_capacity;
            let written = {
                let mut scratch = Scratch::new(&mut buffers.scratch);
                let mut sink =
                    FrameSink::new(&mut buffers.back, &mut buffers.observations, capacity);
                match processor.process(&frame, &mut scratch, &mut sink) {
                    Ok(()) => sink.written(),
                    Err(_) => return StageOutcome::Missed(BypassCause::Refused),
                }
            };
            if written != positions {
                return StageOutcome::Missed(BypassCause::MalformedResult);
            }
            StageOutcome::Produced
        }
        Running::Supervised(supervised) => {
            buffers.back.clear();
            match supervised.exchange(
                &buffers.front,
                position,
                downstream,
                stage.policy.deadline_frames(),
                &mut buffers.back,
            ) {
                Exchange::Produced => StageOutcome::Produced,
                Exchange::Missed(cause) => StageOutcome::Missed(cause),
            }
        }
    }
}

// --------------------------------------------------------------------------- journal ----

/// The bounded transition queue (`docs/specs/call-dsp-graph.md` §5.3).
///
/// Drops its **oldest** entries at capacity, and counts them: a transition is a fact about the
/// past, and the newest are the ones a caller can still act on. That is the seam's own drop-oldest
/// end rather than a second policy.
/// Superseded parameter state coalesces (§5.3, `M-67`); nothing else does. An application moving a
/// stage's parameters faster than it reads its events must not be able to push an activation, a
/// bypass or a teardown out of a bounded queue with facts about a value it has itself replaced.
///
/// **Every write wakes a reader**, because the signal lives here rather than at the call sites
/// (`M-67`). `M-68` journals a bypass in the middle of [`Generation::run`], where the stage that
/// missed its budget is known and where two stages missing on one frame are two entries rather than
/// one; if waking were the caller's job, that path would have to remember to do it and a third
/// producer would have to remember again. Tripping the signal is a flag store and a waker wake, so
/// the media worker may do it on the live path (§6.3).
struct Journal {
    entries: VecDeque<GraphTransition>,
    capacity: usize,
    dropped: u64,
    superseded: u64,
    /// Tripped by every [`Self::push`], and replaced by a reader that found the queue empty.
    recorded: Arc<Stop>,
}

/// How much the journal holds, not what it holds (`M-68`).
///
/// A transition carries no audio, so this is the milder half of the rule: what listing the entries
/// would make configuration-dependent is the record's *length*, and the queue is up to 4,096 deep.
///
/// `recorded` is omitted for the reason `Slot`'s `torn` is: it is a notification primitive that
/// renders longer than everything else here together, and what a reader wants from it — whether a
/// transition has been recorded — is `entries` already.
#[expect(
    clippy::missing_fields_in_debug,
    reason = "`recorded` renders longer than the rest together and says what `entries` says"
)]
impl std::fmt::Debug for Journal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Journal")
            .field("entries", &self.entries.len())
            .field("capacity", &self.capacity)
            .field("dropped", &self.dropped)
            .field("superseded", &self.superseded)
            .finish()
    }
}

impl Journal {
    fn new(capacity: u32) -> Self {
        let capacity = capacity as usize;
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
            dropped: 0,
            superseded: 0,
            recorded: Arc::new(Stop::default()),
        }
    }

    fn push(&mut self, transition: GraphTransition) {
        // §5.3: one stage of one generation has one current parameter state, so an earlier
        // `Configured` for that same stage is a fact about a value the application has already
        // replaced. It gives way to the newer one rather than to the queue's bound — which is what
        // keeps the transitions a caller cannot reconstruct from anything else.
        if let GraphTransition::Configured {
            generation,
            processor,
            ..
        } = transition
        {
            let superseded = |entry: &GraphTransition| {
                matches!(
                    entry,
                    GraphTransition::Configured {
                        generation: earlier,
                        processor: stage,
                        ..
                    } if *earlier == generation && *stage == processor
                )
            };
            if let Some(at) = self.entries.iter().position(superseded) {
                // discard: coalesced into the entry replacing it, counted rather than silent.
                if self.entries.remove(at).is_some() {
                    self.superseded = self.superseded.saturating_add(1);
                }
            }
        }
        while self.entries.len() >= self.capacity {
            // discard: §5.3's drop-oldest end, counted rather than silent.
            match self.entries.pop_front() {
                Some(_) => self.dropped = self.dropped.saturating_add(1),
                None => return,
            }
        }
        self.entries.push_back(transition);
        self.recorded.stop();
    }

    fn drain(&mut self) -> Vec<GraphTransition> {
        self.entries.drain(..).collect()
    }

    /// A signal that will be tripped by the next [`Self::push`], for a reader that found no entry.
    ///
    /// The signal is **replaced** rather than reused, so a wait never returns immediately on a
    /// transition an earlier reader has already taken. Handing it out and arming it is one step for
    /// the same reason recording and waking is one: two steps with a take between them is where the
    /// missed wake would live.
    fn arm(&mut self) -> Arc<Stop> {
        self.recorded = Arc::new(Stop::default());
        Arc::clone(&self.recorded)
    }
}

// ------------------------------------------------------------------------------ slot ----

/// One direction's graph, its journal and the signal its teardown trips.
struct Slot {
    live: Option<Live>,
    journal: Journal,
    generation: u64,
    /// Generations awaiting a reap, so a replacement never blocks the media path on a join.
    retired: Vec<Live>,
    /// Tripped when the live generation ends. Replaced by a fresh one at every activation, so a
    /// barrier always waits on the teardown of the generation it asked about.
    torn: Arc<Stop>,
    /// A stage configured fail-closed failed: this direction carries no audio until a new graph is
    /// installed (§6.1).
    failed_closed: bool,
    /// The session stopped: no new graph may attach.
    stopped: bool,
    /// What every generation installed here has been sized and prepared for.
    ///
    /// Kept on the **slot** rather than on the handle because a graph outlives the session
    /// generation that first attached it (`MediaSession::reconfigure`), and a handle that
    /// remembered the sizing itself would go on offering a stale one to
    /// [`super::DspGraph::replace`] for the rest of the call.
    sizing: Option<Sizing>,
    /// Everything the runtime has observed about this direction's processors (`M-68`).
    ///
    /// Cumulative for the life of the call and never reset by a replacement or a teardown, so a
    /// caller can still read why a graph ended after it has ended.
    counters: GraphCounters,
}

/// The format and frame one generation of a graph was validated, prepared and sized for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sizing {
    format: StreamFormat,
    frame_samples: usize,
}

/// What the slot holds, in the shape a diagnostic wants it (`M-68`).
///
/// `torn` is omitted: it is a notification primitive whose rendering is longer than everything else
/// here together, and what a reader wants from it — whether this direction still has a graph — is
/// `live` already.
#[expect(
    clippy::missing_fields_in_debug,
    reason = "`torn` renders longer than everything else together and says what `live` says"
)]
impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Slot")
            .field("live", &self.live)
            .field("journal", &self.journal)
            .field("generation", &self.generation)
            .field("retired", &self.retired.len())
            .field("failed_closed", &self.failed_closed)
            .field("stopped", &self.stopped)
            .field("sizing", &self.sizing)
            .field("counters", &self.counters)
            .finish()
    }
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            live: None,
            journal: Journal::new(DEFAULT_OBSERVATION_CAPACITY),
            generation: 0,
            retired: Vec::new(),
            torn: Arc::new(Stop::default()),
            failed_closed: false,
            stopped: false,
            sizing: None,
            counters: GraphCounters::default(),
        }
    }
}

impl Slot {
    /// Retire the live generation: cancel every stage and trip the teardown signal.
    fn retire(&mut self, cause: TeardownCause) {
        let Some(mut outgoing) = self.live.take() else {
            return;
        };
        outgoing.cancel();
        self.counters.teardowns = self.counters.teardowns.saturating_add(1);
        if matches!(cause, TeardownCause::FailedClosed { .. }) {
            self.counters.terminal_failures = self.counters.terminal_failures.saturating_add(1);
        }
        self.journal.push(GraphTransition::TornDown {
            generation: outgoing.generation,
            at_position: outgoing.position,
            cause,
        });
        self.retired.push(outgoing);
        self.torn.stop();
    }

    /// The counters, with the journal's own two figures folded in.
    ///
    /// The journal counts what it dropped and what it coalesced because it is the thing that did
    /// both; reading them from there rather than keeping a second copy is what stops the two from
    /// disagreeing. They are also the pair a caller reads together: entries lost to the bound are a
    /// reason to read faster, and entries coalesced under §5.3 are not (`M-67`).
    fn counters(&self) -> GraphCounters {
        GraphCounters {
            transitions_dropped: self.journal.dropped,
            transitions_coalesced: self.journal.superseded,
            ..self.counters
        }
    }
}

// ---------------------------------------------------------------------------- the tap ----

/// A handle on one direction's slot, independent of which session generation owns it.
///
/// One more indirection than it looks like it needs, for the same reason the seam's queues have
/// one: a graph belongs to the **call**, and `MediaSession::reconfigure` builds a whole new session
/// around it. The slot is what a [`super::DspGraph`] and both media loops share, so a renegotiation
/// hands the replacement generation the *same* slot rather than copying its contents out from under
/// a handle that is still pointing at it — and the retired generation's own teardown then finds an
/// empty one, which is what stops a re-INVITE from tearing down the graph it just carried over.
#[derive(Clone)]
pub(crate) struct SlotRef {
    slot: Arc<Mutex<Slot>>,
    direction: AudioDirection,
}

/// The slot's own rendering, reached without ever waiting for it (`M-68`).
///
/// `try_lock` rather than [`hold`]: a diagnostic must never be the thing that blocks the media
/// worker, and "somebody is running a frame right now" is a fact worth reporting rather than a
/// reason to park. It also makes this safe from inside a `with`, which a blocking one would
/// deadlock in.
impl std::fmt::Debug for SlotRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut record = f.debug_struct("SlotRef");
        record.field("direction", &self.direction);
        match self.slot.try_lock() {
            Ok(slot) => record.field("slot", &*slot),
            Err(_) => record.field("slot", &"in use"),
        }
        .finish()
    }
}

impl SlotRef {
    fn new(direction: AudioDirection) -> Self {
        Self {
            slot: Arc::new(Mutex::new(Slot::default())),
            direction,
        }
    }

    /// Take this direction's slot for the length of one operation.
    ///
    /// Every read and every change goes through here, which is what makes "one frame sees one
    /// generation, entire" a property of the type rather than of each call site remembering it.
    fn with<R>(&self, act: impl FnOnce(&mut Slot) -> R) -> R {
        let mut slot = hold(&self.slot);
        act(&mut slot)
    }

    /// Which side of the call this slot is for.
    pub(crate) const fn direction(&self) -> AudioDirection {
        self.direction
    }

    /// Validate a plan whole and publish it, atomically, as a new generation (§3.1, §5.1).
    pub(crate) fn install(
        &self,
        plan: GraphPlan,
        format: StreamFormat,
        frame_samples: usize,
        replacing: bool,
    ) -> Result<u64, GraphError> {
        if plan.direction != self.direction {
            return Err(GraphError::DirectionInUse {
                direction: plan.direction,
            });
        }
        let capacity = plan.bounds.observation_capacity();

        // Refused before anything is built, so a plan that was never going to be admitted does not
        // spawn a worker on its way to being rejected.
        self.with(|slot| admissible(slot, self.direction, replacing))?;
        let mut built = build(plan, format, frame_samples)?;

        self.with(|slot| {
            if let Err(refusal) = admissible(slot, self.direction, replacing) {
                // The window between the two checks is another thread's stop or attach. Whatever
                // was built for this plan is retired rather than leaked, and reaped at the barrier.
                built.cancel();
                slot.retired.push(built);
                return Err(refusal);
            }
            // Recorded on the slot only once the plan is going in, so a refused install cannot
            // change what a later replacement will be sized for.
            slot.sizing = Some(Sizing {
                format,
                frame_samples,
            });
            let generation = slot.generation.saturating_add(1);
            slot.generation = generation;
            built.generation = generation;
            let processors = u32::try_from(built.stages.len()).unwrap_or(u32::MAX);
            if slot.journal.capacity != capacity as usize {
                slot.journal = Journal::new(capacity);
            }

            // The outgoing generation's retained audio belongs to an epoch that no longer exists,
            // so it is discarded rather than flushed (§5.4). Both the swap and the cancellation
            // happen under the one take a frame also needs, so no frame ever sees half of this.
            let transition = match slot.live.replace(built) {
                Some(mut outgoing) => {
                    outgoing.cancel();
                    let previous = outgoing.generation;
                    slot.retired.push(outgoing);
                    GraphTransition::Replaced {
                        generation,
                        previous,
                        at_position: 0,
                        processors,
                    }
                }
                None => GraphTransition::Activated {
                    generation,
                    at_position: 0,
                    processors,
                },
            };
            slot.journal.push(transition);
            slot.failed_closed = false;
            slot.torn = Arc::new(Stop::default());
            Ok(generation)
        })
    }

    /// Run one frame through whatever graph is live, in place.
    ///
    /// Called from the media worker at `M-54`'s tap points and nowhere else (§2). Never awaits,
    /// never blocks on anything but this call's own graph lock, and never allocates.
    fn run(&self, samples: &mut Vec<i16>) {
        self.with(|slot| {
            if slot.failed_closed {
                // §6.1: the audio a fail-closed stage was protecting does not flow. Silenced rather
                // than shortened, because the packetisation and the send clock are the session's
                // and a short frame would be a second decision this story has not made.
                samples.fill(0);
                return;
            }
            // Destructured so the frame's counters, its journal and the generation running it are
            // three borrows of one take rather than three takes.
            let Slot {
                live,
                counters,
                journal,
                ..
            } = slot;
            let Some(generation) = live.as_mut() else {
                return;
            };
            // `M-68`'s signature, and it subsumes `M-67`'s: a bypass is journalled inside `run`,
            // where the stage that missed is known and where two stages missing on one frame are
            // two entries rather than the one an `Option` in the report could carry. The wake that
            // `M-67` needs comes with it, because tripping the signal is `Journal::push`'s job now.
            let report = generation.run(self.direction, samples, counters, journal);
            if let Some(processor) = report.failed_closed {
                slot.retire(TeardownCause::FailedClosed { processor });
                slot.failed_closed = true;
                samples.fill(0);
            }
        });
    }

    /// Tell this direction's graph that the session lost audio it never saw (§6.2).
    fn note_loss(&self, positions: u64) {
        self.with(|slot| {
            if let Some(generation) = slot.live.as_mut() {
                generation.position = generation.position.saturating_add(positions);
                generation.pending = Some(merge(generation.pending, DiscontinuityKind::Loss));
            }
        });
    }

    /// Discard every stage's sample memory and reopen the epoch at position 0 (§6.2).
    fn realign(&self) {
        self.with(|slot| {
            let Slot { live, counters, .. } = slot;
            if let Some(generation) = live.as_mut() {
                generation.realign(DiscontinuityKind::Realign, counters);
            }
        });
    }

    /// Drain this direction's typed transitions.
    pub(crate) fn transitions(&self) -> Vec<GraphTransition> {
        self.with(|slot| slot.journal.drain())
    }

    /// Drain this direction's typed transitions, waiting for one if there is none (§10.3).
    ///
    /// Never a poll and never a duration: the signal is installed and handed out **under the same
    /// take** that found the journal empty, so a transition recorded between the two cannot be
    /// missed. Returns empty only if the future is dropped, which it cannot report.
    pub(crate) async fn next_transitions(&self) -> Vec<GraphTransition> {
        loop {
            let waiting = self.with(|slot| {
                let drained = slot.journal.drain();
                if drained.is_empty() {
                    // A fresh signal, so this waits on a trip that is still to come rather than on
                    // one an earlier drain already consumed.
                    return Err(slot.journal.arm());
                }
                Ok(drained)
            });
            match waiting {
                Ok(drained) => return drained,
                Err(signal) => signal.wait().await,
            }
        }
    }

    /// Move one live stage's parameters, whole, at a position boundary (§10.2).
    ///
    /// Validated against the stage's declared schema **before** anything is assigned, on the
    /// caller's thread and never on the media worker. The take is the same one a frame needs, so
    /// the new values are in force from the first position of the next frame and no frame ever sees
    /// half a set.
    pub(crate) fn configure(
        &self,
        generation: u64,
        index: usize,
        parameters: &[Parameter],
    ) -> Result<ParameterUpdate, GraphError> {
        self.with(|slot| {
            let Some(live) = slot.live.as_mut() else {
                // `0` is §5.1's "no graph": a direction with nothing attached refuses an update
                // rather than inventing a generation for it.
                return Err(GraphError::StaleGeneration {
                    expected: generation,
                    live: 0,
                });
            };
            if live.generation != generation {
                return Err(GraphError::StaleGeneration {
                    expected: generation,
                    live: live.generation,
                });
            }
            let processors = u32::try_from(live.stages.len()).unwrap_or(u32::MAX);
            let at_position = live.position;
            let Some(stage) = live.stages.get_mut(index) else {
                return Err(GraphError::UnknownProcessor {
                    index: u32::try_from(index).unwrap_or(u32::MAX),
                    processors,
                });
            };
            let processor = stage.capability.id();

            // The whole set against the declared schema first. A refusal here has provably assigned
            // nothing, whatever the processor's own `configure` would have done with it.
            stage
                .capability
                .validate_parameters(parameters)
                .map_err(|source| GraphError::Parameter { processor, source })?;
            match &mut stage.kind {
                Running::Inline(inline) => inline
                    .configure(parameters)
                    .map_err(|source| GraphError::Parameter { processor, source })?,
                // §7.4 frames `Hello`, `Frame` and `Result` and nothing else, so there is no
                // message to carry this. Refused rather than accepted and dropped.
                Running::Supervised(_) => {
                    return Err(GraphError::NotConfigurable { processor });
                }
            }

            slot.journal.push(GraphTransition::Configured {
                generation,
                at_position,
                processor,
            });
            Ok(ParameterUpdate {
                generation,
                at_position,
                processor,
            })
        })
    }

    /// Everything the runtime has observed about this direction's processors (`M-68`).
    pub(crate) fn counters(&self) -> GraphCounters {
        self.with(|slot| slot.counters())
    }

    /// Move this direction's graph onto the session generation that just replaced its own
    /// (`MediaSession::reconfigure`).
    ///
    /// A graph belongs to the call, so an ordinary renegotiation re-anchors it: the epoch reopens
    /// at position 0 with every stage reset, and the application does not have to re-attach. A
    /// renegotiation that changed the **audio format or the packetisation** is the one case that
    /// cannot be re-anchored, because both are things the chain was validated, prepared and sized
    /// against. It is torn down with [`TeardownCause::FormatChanged`], which the application reads
    /// off [`Self::transitions`] or waits for on `settled()`.
    pub(crate) fn re_anchor(&self, format: Option<StreamFormat>, frame_samples: usize) {
        let sizing = format.map(|format| Sizing {
            format,
            frame_samples,
        });
        let carried_over = self.with(|slot| {
            // A slot with no graph has nothing to carry and nothing to tear down; recording the
            // new sizing is all it needs, so that a later attach through the handle is sized for
            // the session that is actually running.
            let unchanged = slot.live.is_none() || (slot.sizing == sizing && sizing.is_some());
            slot.sizing = sizing;
            unchanged
        });
        if carried_over {
            self.realign();
            return;
        }
        self.cancel(TeardownCause::FormatChanged);
    }

    /// Validate and publish a new chain, sized as the **session** is rather than as the handle
    /// remembers (§5.4).
    ///
    /// # Errors
    ///
    /// [`GraphError::Detached`] for a slot no graph was ever installed into, and otherwise
    /// whatever [`Self::install`] refuses.
    pub(crate) fn reinstall(&self, plan: GraphPlan) -> Result<u64, GraphError> {
        let Some(Sizing {
            format,
            frame_samples,
        }) = self.with(|slot| slot.sizing)
        else {
            return Err(GraphError::Detached);
        };
        self.install(plan, format, frame_samples, true)
    }

    /// What this direction's graph is holding right now, live and retired generations together.
    pub(crate) fn barrier(&self) -> GraphBarrier {
        self.with(|slot| {
            slot.retired
                .iter()
                .chain(slot.live.as_ref())
                .map(Live::barrier)
                .fold(GraphBarrier::default(), GraphBarrier::add)
        })
    }

    /// The processes this direction's live supervised stages run in, in chain order (§7).
    pub(crate) fn worker_pids(&self) -> Vec<u32> {
        self.with(|slot| {
            slot.live
                .as_ref()
                .map(Live::worker_pids)
                .unwrap_or_default()
        })
    }

    /// The generation currently live, or 0 for none.
    pub(crate) fn generation(&self) -> u64 {
        self.with(|slot| slot.live.as_ref().map_or(0, |live| live.generation))
    }

    /// Whether this graph may claim that over-budget work cannot stall RTP (§3.3).
    pub(crate) fn contains_overrun(&self) -> bool {
        self.with(|slot| slot.live.as_ref().is_some_and(|live| live.contains_overrun))
    }

    /// The sum of the chain's declared algorithmic latency, in positions (§7.1).
    pub(crate) fn latency_positions(&self) -> u64 {
        self.with(|slot| slot.live.as_ref().map_or(0, |live| live.latency_positions))
    }

    /// The pipeline lag the supervised stages add, in frames (§7.1).
    pub(crate) fn pipeline_frames(&self) -> u32 {
        self.with(|slot| slot.live.as_ref().map_or(0, |live| live.pipeline_frames))
    }

    /// Cancel this direction's graph and every generation it retired, without waiting.
    ///
    /// The synchronous half of the barrier, so it is safe from `stop()` and from `Drop`. The reap
    /// is [`Self::settle`]'s, because joining a worker is an await.
    pub(crate) fn cancel(&self, cause: TeardownCause) {
        self.with(|slot| {
            slot.retire(cause);
            for retired in &mut slot.retired {
                retired.cancel();
            }
            if matches!(cause, TeardownCause::SessionStopped) {
                slot.stopped = true;
            }
            slot.torn.stop();
        });
    }

    /// Reap every worker of every retired generation, then report the barrier (§8).
    ///
    /// Awaits an event and never a duration, so a stopped session's teardown answers rather than
    /// holding a runtime worker nothing can reclaim.
    pub(crate) async fn settle(&self) -> GraphBarrier {
        loop {
            let mut retired = self.with(|slot| std::mem::take(&mut slot.retired));
            if retired.is_empty() {
                break;
            }
            for generation in &mut retired {
                generation.reap().await;
            }
        }
        self.barrier()
    }

    /// Wait until this direction's graph has been torn down, then reap it (§8).
    pub(crate) async fn settled(&self) -> GraphBarrier {
        loop {
            // Each activation installs a fresh signal, so this always waits on the teardown of the
            // generation that was live when it looked — never on a stale one, and never in a spin.
            let torn = self.with(|slot| slot.live.as_ref().map(|_| Arc::clone(&slot.torn)));
            let Some(torn) = torn else { break };
            torn.wait().await;
        }
        self.settle().await
    }
}

/// Whether a direction will take a plan at all, before one is built for it.
fn admissible(slot: &Slot, direction: AudioDirection, replacing: bool) -> Result<(), GraphError> {
    if slot.stopped {
        return Err(GraphError::SessionStopped);
    }
    if !replacing && slot.live.is_some() {
        return Err(GraphError::DirectionInUse { direction });
    }
    Ok(())
}

/// Which slot each direction currently is.
#[derive(Debug, Clone)]
struct Directions {
    inbound: SlotRef,
    outbound: SlotRef,
}

impl Default for Directions {
    fn default() -> Self {
        Self {
            inbound: SlotRef::new(AudioDirection::Inbound),
            outbound: SlotRef::new(AudioDirection::Outbound),
        }
    }
}

/// Every DSP graph one call owns (`docs/specs/call-dsp-graph.md` §3).
///
/// One per session, holding at most one graph per direction. Nothing here is shared between two
/// calls: a graph belongs to one direction of one call, its processors are owned by it, its buffers
/// are owned by it and its workers are owned by it. Two calls running the same processor kind run
/// two instances, which is a consequence of ownership rather than a rule anyone has to observe.
#[derive(Debug, Default)]
pub(crate) struct CallDsp {
    directions: Mutex<Directions>,
}

impl CallDsp {
    /// This session's handle on one direction's slot.
    pub(crate) fn slot(&self, direction: AudioDirection) -> SlotRef {
        let directions = hold(&self.directions);
        match direction {
            AudioDirection::Inbound => directions.inbound.clone(),
            AudioDirection::Outbound => directions.outbound.clone(),
        }
    }

    /// Run one frame of one direction through whatever graph is live, in place.
    pub(crate) fn run(&self, direction: AudioDirection, samples: &mut Vec<i16>) {
        self.slot(direction).run(samples);
    }

    /// Tell a direction's graph that the session lost audio it never saw (§6.2).
    pub(crate) fn note_loss(&self, direction: AudioDirection, positions: u64) {
        self.slot(direction).note_loss(positions);
    }

    /// Carry a retired session generation's graphs onto this one (`MediaSession::reconfigure`).
    ///
    /// Graphs belong to the call and not to a worker generation, exactly as seam attachments do, so
    /// a re-INVITE must not make an application re-attach. The slots are **moved**: the retired
    /// generation is left with empty ones, so its own shutdown tears down nothing that is still in
    /// use. The move re-anchors both directions — audio queued under a media generation that no
    /// longer exists would land in the new epoch as old audio at a new position.
    ///
    /// `format` and `frame_samples` are the **replacement** session's, and a graph is carried over
    /// only if they are the ones it was validated against. One that was not is torn down with
    /// [`TeardownCause::FormatChanged`] rather than carried onto audio it never agreed to
    /// (`M-68`); see [`SlotRef::re_anchor`].
    pub(crate) fn adopt(
        &self,
        previous: &Self,
        format: Option<StreamFormat>,
        frame_samples: usize,
    ) {
        let carried = std::mem::take(&mut *hold(&previous.directions));
        *hold(&self.directions) = carried;
        for direction in [AudioDirection::Inbound, AudioDirection::Outbound] {
            self.slot(direction).re_anchor(format, frame_samples);
        }
    }

    /// Cancel both directions.
    pub(crate) fn cancel_all(&self, cause: TeardownCause) {
        self.slot(AudioDirection::Inbound).cancel(cause);
        self.slot(AudioDirection::Outbound).cancel(cause);
    }

    /// Reap both directions' retired generations.
    pub(crate) async fn settle_all(&self) {
        self.slot(AudioDirection::Inbound).settle().await;
        self.slot(AudioDirection::Outbound).settle().await;
    }
}

// --------------------------------------------------------------------------- building ----

/// Validate a plan whole, then prepare and allocate everything it will ever use (§3.1).
fn build(plan: GraphPlan, format: StreamFormat, frame_samples: usize) -> Result<Live, GraphError> {
    let GraphPlan {
        direction,
        bounds,
        stages,
        workspace,
    } = plan;
    // Every declaration is checked before one processor is prepared, so a refusal in the middle of
    // a chain never leaves the front of it half-attached, and no worker is spawned on the way to
    // being rejected.
    let admitted = admit(&stages, &bounds, format, workspace)?;

    // Only now, with the whole plan admitted, is anything prepared, allocated or spawned.
    let running = prepare(stages, &bounds, direction, format, frame_samples)?;

    Ok(Live {
        generation: 0,
        format,
        stages: running,
        buffers: Buffers {
            front: Vec::with_capacity(frame_samples),
            back: Vec::with_capacity(frame_samples),
            scratch: vec![0; bounds.scratch_samples() as usize],
            observations: Vec::with_capacity(bounds.observation_capacity() as usize),
            observation_capacity: bounds.observation_capacity(),
        },
        position: 0,
        frame_samples,
        pending: None,
        contains_overrun: admitted.contains_overrun,
        latency_positions: admitted.latency_positions,
        pipeline_frames: admitted.pipeline_frames,
    })
}

/// What checking a whole plan established about the chain it describes.
#[derive(Debug, Clone, Copy)]
struct Admitted {
    contains_overrun: bool,
    latency_positions: u64,
    pipeline_frames: u32,
}

/// Check every stage of a plan against the bounds, the format and §3.2's doors (§3.1).
fn admit(
    stages: &[Planned],
    bounds: &GraphBounds,
    format: StreamFormat,
    workspace: bool,
) -> Result<Admitted, GraphError> {
    bounds.validate()?;
    let count = u32::try_from(stages.len()).unwrap_or(u32::MAX);
    if count > bounds.max_processors() {
        return Err(GraphError::TooManyProcessors {
            limit: bounds.max_processors(),
        });
    }

    let mut contains_overrun = true;
    let mut latency_positions = 0u64;
    let mut pipeline_frames = 0u32;
    for planned in stages {
        let capability = planned.capability();
        let id = capability.id();
        capability
            .validate()
            .map_err(|source| GraphError::Capability {
                processor: id,
                source,
            })?;
        capability
            .accepts_format(format)
            .map_err(|source| GraphError::Format {
                processor: id,
                source,
            })?;
        let profile = capability.execution().profile();
        match planned {
            // §3.2: the public door admits neither the proven profile — an application cannot add
            // to this repository's gate — nor the supervised one, which is not run on the media
            // worker and therefore does not arrive through an inline stage.
            Planned::Inline {
                workspace: provenance,
                ..
            } if matches!(profile, ExecutionProfile::SupervisedIsolated)
                || (matches!(profile, ExecutionProfile::ProvenInline)
                    && !(workspace || *provenance)) =>
            {
                return Err(GraphError::ProfileNotAdmissible {
                    processor: id,
                    profile,
                });
            }
            Planned::Supervised(_) if !matches!(profile, ExecutionProfile::SupervisedIsolated) => {
                return Err(GraphError::NotSupervised {
                    processor: id,
                    profile,
                });
            }
            Planned::Supervised(_) => {
                pipeline_frames =
                    pipeline_frames.saturating_add(capability.execution().deadline_frames());
            }
            Planned::Inline { .. } => {}
        }
        if !matches!(capability.length(), LengthPolicy::Preserving) {
            return Err(GraphError::LengthNotPreserving { processor: id });
        }
        if capability.max_frame_samples() > bounds.max_frame_samples() {
            return Err(GraphError::FrameExceedsBound {
                processor: id,
                declared: capability.max_frame_samples(),
                bound: bounds.max_frame_samples(),
            });
        }
        if capability.scratch_samples() > bounds.scratch_samples() {
            return Err(GraphError::ScratchExceedsBound {
                processor: id,
                declared: capability.scratch_samples(),
                bound: bounds.scratch_samples(),
            });
        }
        let retained = capability
            .tail_positions()
            .max(capability.latency_positions());
        if retained > bounds.retained_tail_positions() {
            return Err(GraphError::TailExceedsBound {
                processor: id,
                declared: retained,
                bound: bounds.retained_tail_positions(),
            });
        }
        contains_overrun &= profile.contains_overrun();
        latency_positions =
            latency_positions.saturating_add(u64::from(capability.latency_positions()));
    }
    Ok(Admitted {
        contains_overrun,
        latency_positions,
        pipeline_frames,
    })
}

/// Prepare every stage and spawn whatever a supervised one owns (§3.1).
fn prepare(
    stages: Vec<Planned>,
    bounds: &GraphBounds,
    direction: AudioDirection,
    format: StreamFormat,
    frame_samples: usize,
) -> Result<Vec<Stage>, GraphError> {
    let mut running = Vec::with_capacity(stages.len());
    for planned in stages {
        let capability = planned.capability();
        let policy = capability.execution();
        let kind = match planned {
            Planned::Inline {
                mut processor,
                workspace: _,
            } => {
                processor
                    .prepare(direction, format)
                    .map_err(|source| GraphError::Format {
                        processor: capability.id(),
                        source,
                    })?;
                Running::Inline(processor)
            }
            Planned::Supervised(worker) => Running::Supervised(Box::new(
                Supervised::spawn(
                    &worker,
                    bounds.worker_queue_capacity(),
                    frame_samples,
                    direction,
                    format,
                )
                .map_err(|error| GraphError::WorkerSpawn {
                    processor: capability.id(),
                    error: error.kind(),
                })?,
            )),
        };
        running.push(Stage {
            capability,
            policy,
            kind,
            misses: 0,
            bypassed: false,
        });
    }
    Ok(running)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use sipx_audio::dsp::{FrameAdmission, Parameter, ParameterError, ProcessError};

    /// The identity, on whichever profile a case needs.
    struct Ident {
        admission: FrameAdmission,
        profile: ExecutionProfile,
    }

    impl Ident {
        fn boxed(profile: ExecutionProfile) -> Box<dyn FrameProcessor + Send> {
            Box::new(Self {
                admission: FrameAdmission::new(),
                profile,
            })
        }
    }

    impl FrameProcessor for Ident {
        fn capability(&self) -> DspCapability {
            DspCapability::new("ident").with_execution(ExecutionPolicy::new(self.profile))
        }
        fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
            self.capability().validate_parameters(parameters)
        }
        fn prepare(
            &mut self,
            direction: AudioDirection,
            format: StreamFormat,
        ) -> Result<(), FormatError> {
            self.admission
                .prepare(&self.capability(), direction, format)
        }
        fn process(
            &mut self,
            frame: &DspFrame<'_>,
            _scratch: &mut Scratch<'_>,
            sink: &mut FrameSink<'_>,
        ) -> Result<(), ProcessError> {
            self.admission.admit(&self.capability(), frame)?;
            sink.write(frame.samples())
        }
        fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
            self.admission.admit_flush()
        }
        fn reset(&mut self, _cause: DspResetCause) {
            self.admission.reset();
        }
        fn cancel(&mut self) {
            self.admission.cancel();
        }
        fn retained(&self) -> u32 {
            0
        }
    }

    fn narrowband() -> StreamFormat {
        StreamFormat::new(8_000, 1).unwrap()
    }

    /// §3.2: the proven profile is admitted through the crate-internal door and refused through the
    /// public one. Both halves, because a rule with only its refusal tested is a rule nothing has
    /// shown a way past.
    #[test]
    fn the_proven_profile_is_admitted_only_from_the_workspace() {
        let application = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
            .with_processor(Ident::boxed(ExecutionProfile::ProvenInline));
        assert_eq!(
            build(application, narrowband(), 160).err(),
            Some(GraphError::ProfileNotAdmissible {
                processor: "ident",
                profile: ExecutionProfile::ProvenInline,
            })
        );

        let workspace = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
            .with_workspace_provenance()
            .with_processor(Ident::boxed(ExecutionProfile::ProvenInline));
        let built = build(workspace, narrowband(), 160).expect("the workspace door admits it");
        assert!(
            built.contains_overrun,
            "a proven-inline chain may claim its work is bounded by construction"
        );
    }

    /// §3.3: a graph is only as contained as its least contained stage, and no configuration
    /// reaches that.
    #[test]
    fn one_cooperative_stage_makes_the_whole_chain_uncontained() {
        let mixed = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
            .with_workspace_provenance()
            .with_processor(Ident::boxed(ExecutionProfile::ProvenInline))
            .with_processor(Ident::boxed(ExecutionProfile::TrustedCooperativeNative));
        let built = build(mixed, narrowband(), 160).expect("both stages are admissible");
        assert!(!built.contains_overrun);
    }

    /// §3.2: a supervised declaration cannot reach a graph through the inline door, where it would
    /// be run on the media worker under a profile that says it never is.
    #[test]
    fn a_supervised_declaration_cannot_enter_through_the_inline_door() {
        let plan = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
            .with_workspace_provenance()
            .with_processor(Ident::boxed(ExecutionProfile::SupervisedIsolated));
        assert_eq!(
            build(plan, narrowband(), 160).err(),
            Some(GraphError::ProfileNotAdmissible {
                processor: "ident",
                profile: ExecutionProfile::SupervisedIsolated,
            })
        );
    }

    /// §3: a second attach to one direction is refused rather than becoming a replacement nobody
    /// asked for.
    #[test]
    fn a_direction_takes_one_graph() {
        let dsp = CallDsp::default();
        let slot = dsp.slot(AudioDirection::Outbound);
        let first = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
            .with_processor(Ident::boxed(ExecutionProfile::TrustedCooperativeNative));
        assert_eq!(slot.install(first, narrowband(), 160, false), Ok(1));

        let second = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
            .with_processor(Ident::boxed(ExecutionProfile::TrustedCooperativeNative));
        assert_eq!(
            slot.install(second, narrowband(), 160, false),
            Err(GraphError::DirectionInUse {
                direction: AudioDirection::Outbound,
            })
        );
        assert_eq!(slot.generation(), 1, "the first graph is untouched");

        // And a plan for the other direction cannot be installed into this slot by mistake.
        let crossed = GraphPlan::new(AudioDirection::Inbound, GraphBounds::new());
        assert_eq!(
            slot.install(crossed, narrowband(), 160, true),
            Err(GraphError::DirectionInUse {
                direction: AudioDirection::Inbound,
            })
        );
    }

    /// §4.3: a stage that may change a frame's length is refused, because the session's
    /// packetisation is fixed and this story defines no re-framing stage.
    #[test]
    fn a_length_changing_stage_is_refused_for_a_live_call() {
        struct Shrinking;
        impl FrameProcessor for Shrinking {
            fn capability(&self) -> DspCapability {
                DspCapability::new("shrinking")
                    .with_length(LengthPolicy::Bounded { max_positions: 4 })
                    .with_execution(ExecutionPolicy::new(
                        ExecutionProfile::TrustedCooperativeNative,
                    ))
            }
            fn configure(&mut self, _parameters: &[Parameter]) -> Result<(), ParameterError> {
                Ok(())
            }
            fn prepare(
                &mut self,
                _direction: AudioDirection,
                _format: StreamFormat,
            ) -> Result<(), FormatError> {
                Ok(())
            }
            fn process(
                &mut self,
                _frame: &DspFrame<'_>,
                _scratch: &mut Scratch<'_>,
                _sink: &mut FrameSink<'_>,
            ) -> Result<(), ProcessError> {
                Ok(())
            }
            fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
                Ok(())
            }
            fn reset(&mut self, _cause: DspResetCause) {}
            fn cancel(&mut self) {}
            fn retained(&self) -> u32 {
                0
            }
        }

        let plan = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
            .with_processor(Box::new(Shrinking));
        assert_eq!(
            build(plan, narrowband(), 160).err(),
            Some(GraphError::LengthNotPreserving {
                processor: "shrinking",
            })
        );
    }

    /// §5.3: the transition queue is bounded and drops its oldest rather than growing.
    #[test]
    fn the_transition_queue_drops_its_oldest() {
        let mut journal = Journal::new(2);
        for generation in 1..=4 {
            journal.push(GraphTransition::Activated {
                generation,
                at_position: 0,
                processors: 1,
            });
        }
        assert_eq!(journal.dropped, 2);
        assert_eq!(
            journal.drain(),
            vec![
                GraphTransition::Activated {
                    generation: 3,
                    at_position: 0,
                    processors: 1,
                },
                GraphTransition::Activated {
                    generation: 4,
                    at_position: 0,
                    processors: 1,
                },
            ]
        );
    }
}
