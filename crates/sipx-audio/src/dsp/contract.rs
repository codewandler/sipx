//! The types of the custom call-DSP contract: `docs/specs/custom-call-dsp.md` §3 through §9.

use crate::analysis::{AudioDirection, DiscontinuityKind};
use crate::pcm::{MAX_SAMPLE_RATE, PcmError};

/// The largest interleaved frame §5 admits, in `i16` samples.
///
/// The ceiling is the §9.2 CPU bound: `process` is `O(positions)` with a fixed per-position step,
/// so a frame ceiling is a hard ceiling on one call's DSP cost. It is
/// [`crate::analysis::MAX_FRAME_SAMPLES`]'s value, deliberately: two processors observing one call
/// through two contracts that disagree about how much audio a frame may carry is how a graph and
/// an analyser end up sized differently for the same seam.
pub const MAX_FRAME_SAMPLES: u32 = 65_536;

/// The largest scratch region §5 admits a processor to declare, in `i16` samples.
pub const MAX_SCRATCH_SAMPLES: u32 = 65_536;

/// The largest algorithmic latency or tail §5 admits, in positions.
pub const MAX_LATENCY_POSITIONS: u32 = 65_536;

/// The most channels §3.2 admits in one interleaved stream.
///
/// The call paths this contract serves are mono and stereo; eight is headroom rather than a plan,
/// chosen so one frame's interleaved sample count and one position's channel stride both stay
/// inside [`MAX_FRAME_SAMPLES`] without a second cap.
pub const MAX_CHANNELS: u8 = 8;

/// The longest deadline §7.4 admits, in frame durations.
pub const MAX_DEADLINE_FRAMES: u32 = 8;

/// The longest run of missed deadlines §7.4 admits before a processor has failed.
pub const MAX_CONSECUTIVE_MISSES: u32 = 64;

/// The most parameters one capability may declare (§5).
pub const MAX_PARAMETERS: usize = 32;

// ------------------------------------------------------------------ stream and frame ----

/// The rate and channel count of one interleaved signed 16-bit stream (§3.2).
///
/// The sample representation is signed 16-bit and nothing else. The seam offers unsigned 8-bit as
/// well; a processor never sees it, because a second depth in the sample path is a second
/// arithmetic, a second saturation boundary and a second set of vectors for every effect. Depth
/// conversion happens at the attachment, before this contract begins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamFormat {
    sample_rate: u32,
    channels: u8,
}

impl Default for StreamFormat {
    /// §12.1's reference stream `D8`: 8,000 Hz, one channel.
    fn default() -> Self {
        Self {
            sample_rate: 8_000,
            channels: 1,
        }
    }
}

impl StreamFormat {
    /// Validate a rate and channel count.
    ///
    /// # Errors
    ///
    /// [`FormatError::UnsupportedRate`] outside the linear-PCM boundary's `1..=384,000` Hz domain,
    /// carrying that boundary's own [`PcmError`] rather than a second refusal type, and
    /// [`FormatError::UnsupportedChannelCount`] outside `1..=`[`MAX_CHANNELS`].
    pub const fn new(sample_rate: u32, channels: u8) -> Result<Self, FormatError> {
        if sample_rate == 0 || sample_rate > MAX_SAMPLE_RATE {
            return Err(FormatError::UnsupportedRate(
                PcmError::UnsupportedSampleRate(sample_rate),
            ));
        }
        if channels == 0 || channels > MAX_CHANNELS {
            return Err(FormatError::UnsupportedChannelCount { channels });
        }
        Ok(Self {
            sample_rate,
            channels,
        })
    }

    /// Samples per second, per channel.
    #[must_use]
    pub const fn sample_rate(self) -> u32 {
        self.sample_rate
    }

    /// How many interleaved channels one position carries.
    #[must_use]
    pub const fn channels(self) -> u8 {
        self.channels
    }
}

/// One frame offered to a processor (§3.3).
///
/// Samples are borrowed for the duration of the call and MUST NOT be retained after
/// [`FrameProcessor::process`] returns. A processor needing sample memory across frames owns that
/// memory as declared state and copies into it; what it may not do is keep the caller's buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DspFrame<'a> {
    direction: AudioDirection,
    format: StreamFormat,
    position: u64,
    discontinuity: Option<DiscontinuityKind>,
    samples: &'a [i16],
}

impl<'a> DspFrame<'a> {
    /// A frame that continues the stream: no break precedes it.
    ///
    /// `position` is the index of the frame's first position within the current epoch, counted at
    /// `format`'s own rate — the seam's `sample_time`, under the seam's epoch rule.
    #[must_use]
    pub const fn new(
        direction: AudioDirection,
        format: StreamFormat,
        position: u64,
        samples: &'a [i16],
    ) -> Self {
        Self {
            direction,
            format,
            position,
            discontinuity: None,
            samples,
        }
    }

    /// Declare the break that immediately precedes this frame.
    ///
    /// The reset it causes runs before this frame's own samples are consumed, so those samples open
    /// the new epoch (§8.1).
    #[must_use]
    pub const fn with_discontinuity(mut self, kind: DiscontinuityKind) -> Self {
        self.discontinuity = Some(kind);
        self
    }

    /// Which side of the call these samples belong to.
    #[must_use]
    pub const fn direction(&self) -> AudioDirection {
        self.direction
    }

    /// The rate and channel count these samples are in.
    #[must_use]
    pub const fn format(&self) -> StreamFormat {
        self.format
    }

    /// The first position's index in the current epoch.
    #[must_use]
    pub const fn position(&self) -> u64 {
        self.position
    }

    /// The break immediately before this frame, if the seam declared one.
    #[must_use]
    pub const fn discontinuity(&self) -> Option<DiscontinuityKind> {
        self.discontinuity
    }

    /// The borrowed interleaved samples, channel 0 first.
    #[must_use]
    pub const fn samples(&self) -> &'a [i16] {
        self.samples
    }

    /// How many positions this frame carries: one sample per channel each.
    ///
    /// Truncating division: a frame whose sample count is not a multiple of its channel count is
    /// refused at admission (§8.3), so an accepted frame's division is exact.
    #[must_use]
    pub const fn positions(&self) -> u64 {
        self.samples.len() as u64 / self.format.channels as u64
    }
}

// ------------------------------------------------------------------------ parameters ----

/// One value a parameter may take (§3.6).
///
/// There is no floating-point variant, and that is the point: "finite parameter state" is a
/// property this vocabulary cannot express the negation of. A NaN gain, an infinite cutoff and a
/// signalling payload smuggled through a bit pattern are not refused at a boundary here — they are
/// unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ParameterValue {
    /// A boolean.
    Flag(bool),
    /// An exact integer.
    Integer(i64),
    /// A ratio in thousandths of a unit: `Ratio(1_500)` is 1.5, and `Ratio(-6_000)` is −6.
    Ratio(i32),
}

/// The closed range a parameter's value must lie in (§3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ParameterDomain {
    /// Either boolean.
    Flag,
    /// An inclusive integer range.
    Integer {
        /// The smallest admitted value.
        min: i64,
        /// The largest admitted value.
        max: i64,
    },
    /// An inclusive range in thousandths.
    Ratio {
        /// The smallest admitted value, in thousandths.
        min: i32,
        /// The largest admitted value, in thousandths.
        max: i32,
    },
}

/// One parameter a processor declares (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParameterSpec {
    id: &'static str,
    domain: ParameterDomain,
}

impl ParameterSpec {
    /// Declare a parameter under a stable identifier.
    #[must_use]
    pub const fn new(id: &'static str, domain: ParameterDomain) -> Self {
        Self { id, domain }
    }

    /// The identifier the SDK selects this parameter by.
    #[must_use]
    pub const fn id(&self) -> &'static str {
        self.id
    }

    /// The closed range values must lie in.
    #[must_use]
    pub const fn domain(&self) -> ParameterDomain {
        self.domain
    }

    /// Whether a value is admissible under this declaration.
    ///
    /// # Errors
    ///
    /// [`ParameterError::KindMismatch`] when the value is a different kind than the domain, and
    /// [`ParameterError::OutOfRange`] when it is the right kind and outside the range.
    pub const fn accepts(&self, value: ParameterValue) -> Result<(), ParameterError> {
        match (self.domain, value) {
            (ParameterDomain::Flag, ParameterValue::Flag(_)) => Ok(()),
            (ParameterDomain::Integer { min, max }, ParameterValue::Integer(found)) => {
                if found < min || found > max {
                    return Err(ParameterError::OutOfRange { id: self.id });
                }
                Ok(())
            }
            (ParameterDomain::Ratio { min, max }, ParameterValue::Ratio(found)) => {
                if found < min || found > max {
                    return Err(ParameterError::OutOfRange { id: self.id });
                }
                Ok(())
            }
            _ => Err(ParameterError::KindMismatch { id: self.id }),
        }
    }
}

/// One parameter assignment offered to a processor (§3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Parameter {
    id: &'static str,
    value: ParameterValue,
}

impl Parameter {
    /// Assign `value` to the parameter `id`.
    #[must_use]
    pub const fn new(id: &'static str, value: ParameterValue) -> Self {
        Self { id, value }
    }

    /// Which declared parameter this assigns.
    #[must_use]
    pub const fn id(&self) -> &'static str {
        self.id
    }

    /// The value assigned.
    #[must_use]
    pub const fn value(&self) -> ParameterValue {
        self.value
    }
}

// ------------------------------------------------------------------ execution profiles ----

/// How a profile keeps over-budget processing away from the RTP loop (§7).
///
/// Two mechanisms, and they are separately true. Naming them apart is what stops "contained" from
/// becoming one word covering two different promises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum OverrunContainment {
    /// The work itself is bounded: no allocation after `prepare`, `O(positions)` with a fixed
    /// per-position step, no loop whose trip count depends on a sample value, no blocking call, and
    /// a declared frame ceiling. Nothing preempts the call because nothing needs to.
    BoundedWork,
    /// The *wait* is bounded: the media worker offers a frame and takes a result only if one is
    /// present by the deadline, so it never waits on the processor at all.
    BoundedWait,
    /// Neither. The processor runs on the media worker and the stack cannot bound what it does
    /// there.
    None,
}

impl OverrunContainment {
    /// Whether this mechanism supports the claim that over-budget work cannot stall RTP.
    #[must_use]
    pub const fn contains(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// What the runtime can actually do when a frame misses its deadline (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DeadlineAction {
    /// The result is abandoned unread and §7.4's failure policy applies to this frame. Only a
    /// profile that does not run the processor on the media worker can do this.
    AbandonResult,
    /// The call is measured once it has returned; this frame's audio is already whatever the
    /// processor made of it, and the policy applies from the next frame. A synchronous call on the
    /// media worker cannot be interrupted, so this is the only honest action for one.
    AccountAfterReturn,
}

/// Where a processor's `process` call runs, and what that placement may promise (§7).
///
/// Sandboxed execution is a fourth profile that does not exist yet and is not silently promised
/// here, which is why this enum is extensible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExecutionProfile {
    /// Crate-owned code whose per-frame work is bounded by construction and proven by the
    /// conformance harness, running synchronously on the media worker.
    ///
    /// **Claims** that over-budget work cannot stall RTP, because there is no unbounded work to go
    /// over budget with; the containment is in the shape of the work.
    ///
    /// **Does not claim** that anything preempts a running call — nothing does — nor that a machine
    /// too slow for the configured graph will meet its deadlines. The bound is on work, not on wall
    /// time. An application-supplied processor may not select this profile: "proven" names evidence
    /// in this repository's gate, and an application cannot add to it.
    ProvenInline,
    /// Application-supplied code in a supervised process the runtime owns, reached through bounded
    /// request and result channels. `process` is never called on the media worker.
    ///
    /// **Claims** that over-budget work cannot stall RTP, because the media worker never waits for
    /// application code: the wait is bounded by the deadline and the declared failure action applies
    /// at its expiry. A hung, looping, crashed or malformed worker costs exactly that action plus a
    /// termination and a reap.
    ///
    /// **Does not claim** that the audio survives. An abandoned result is audio that did not get
    /// processed, so a fail-open bypass here is audible: the containment is of the stall, not of
    /// the artefact. Nor does it bound the isolated process's own memory or CPU beyond what the
    /// operating system is configured to bound.
    SupervisedIsolated,
    /// Trusted application-supplied native code called directly on the media worker, by explicit
    /// selection.
    ///
    /// **Claims nothing about containment.** sipx cannot preempt it, cannot cancel it, cannot reap
    /// it and cannot bound its memory. A callback that does not return stalls the media worker and
    /// therefore stalls RTP for that call, and no configured deadline, failure action or teardown
    /// barrier changes that, because every one of them is code that runs after the callback
    /// returns. The stack validates the declared shape and measures calls that do return; a call
    /// that does not return is outside sipx's containment and teardown guarantees. The profile may
    /// copy audio it was lent, spawn work the stack does not own, and outlive the call.
    ///
    /// Conformance never upgrades it: [`Self::contains_overrun`] is `false` for this profile
    /// unconditionally, and no harness result, measurement or configuration may set it true.
    TrustedCooperativeNative,
}

impl ExecutionProfile {
    /// How this profile keeps an overrun away from RTP, if it does.
    #[must_use]
    pub const fn containment(self) -> OverrunContainment {
        match self {
            Self::ProvenInline => OverrunContainment::BoundedWork,
            Self::SupervisedIsolated => OverrunContainment::BoundedWait,
            Self::TrustedCooperativeNative => OverrunContainment::None,
        }
    }

    /// Whether this profile may claim that over-budget work cannot stall RTP.
    ///
    /// True for exactly [`Self::ProvenInline`] and [`Self::SupervisedIsolated`]. It is not
    /// configurable and no measurement changes it.
    #[must_use]
    pub const fn contains_overrun(self) -> bool {
        self.containment().contains()
    }

    /// What the runtime does with a frame that missed its deadline under this profile.
    #[must_use]
    pub const fn deadline_action(self) -> DeadlineAction {
        match self {
            Self::SupervisedIsolated => DeadlineAction::AbandonResult,
            Self::ProvenInline | Self::TrustedCooperativeNative => {
                DeadlineAction::AccountAfterReturn
            }
        }
    }

    /// Whether `process` runs on the media worker under this profile.
    ///
    /// Two of the three do; only one of those two is permitted to call itself contained, which is
    /// exactly why these are separate questions.
    #[must_use]
    pub const fn runs_on_media_worker(self) -> bool {
        !matches!(self, Self::SupervisedIsolated)
    }
}

/// What happens to a call's audio once a processor has failed (§7.4).
///
/// Both are configured and neither is inferred. A default that guessed would be guessing at whether
/// an application's audio is allowed to leave unprocessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FailureAction {
    /// Fail open: the processor is bypassed and unmodified audio continues to flow. The call keeps
    /// working and sounds wrong. The right default for an effect.
    BypassOpen,
    /// Fail closed: the graph is torn down and the audio it was protecting does not flow. The right
    /// choice when a processor's absence is a policy breach rather than a quality regression — the
    /// redaction case, where unprocessed audio leaving the stack is worse than no audio leaving it.
    TerminateClosed,
}

/// The minimum execution and failure policy every capability carries (§7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExecutionPolicy {
    profile: ExecutionProfile,
    deadline_frames: u32,
    max_consecutive_misses: u32,
    on_failure: FailureAction,
}

impl ExecutionPolicy {
    /// The minimum policy for a profile: a one-frame deadline, three consecutive misses, fail open.
    #[must_use]
    pub const fn new(profile: ExecutionProfile) -> Self {
        Self {
            profile,
            deadline_frames: 1,
            max_consecutive_misses: 3,
            on_failure: FailureAction::BypassOpen,
        }
    }

    /// How many frame durations the result may take. Domain `1..=`[`MAX_DEADLINE_FRAMES`].
    ///
    /// Stated in frame durations rather than milliseconds because the runtime already knows the
    /// rate and the frame size, and the processor must never learn either as a wall-clock quantity:
    /// a processor that could read its own deadline would have a clock.
    #[must_use]
    pub const fn with_deadline_frames(mut self, frames: u32) -> Self {
        self.deadline_frames = frames;
        self
    }

    /// How many deadlines in a row may be missed before the processor has failed. Domain
    /// `1..=`[`MAX_CONSECUTIVE_MISSES`].
    #[must_use]
    pub const fn with_max_consecutive_misses(mut self, misses: u32) -> Self {
        self.max_consecutive_misses = misses;
        self
    }

    /// Whether failure bypasses the processor or tears the graph down.
    #[must_use]
    pub const fn with_on_failure(mut self, action: FailureAction) -> Self {
        self.on_failure = action;
        self
    }

    /// Where this processor's `process` call runs.
    #[must_use]
    pub const fn profile(&self) -> ExecutionProfile {
        self.profile
    }

    /// The configured deadline, in frame durations.
    #[must_use]
    pub const fn deadline_frames(&self) -> u32 {
        self.deadline_frames
    }

    /// The configured run of misses that constitutes failure.
    #[must_use]
    pub const fn max_consecutive_misses(&self) -> u32 {
        self.max_consecutive_misses
    }

    /// The configured fail-open or fail-closed action.
    #[must_use]
    pub const fn on_failure(&self) -> FailureAction {
        self.on_failure
    }

    /// Whether this policy's profile may claim that over-budget work cannot stall RTP.
    #[must_use]
    pub const fn contains_overrun(&self) -> bool {
        self.profile.contains_overrun()
    }
}

// ------------------------------------------------------------------------ capability ----

/// Which sample rates a processor accepts (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RateSupport {
    /// Every rate the linear-PCM boundary admits: `1..=384,000` Hz.
    Any,
    /// Exactly these rates, strictly ascending and non-empty.
    Exactly(&'static [u32]),
}

impl RateSupport {
    /// Whether `rate` is one this processor accepts.
    #[must_use]
    pub fn accepts(self, rate: u32) -> bool {
        match self {
            Self::Any => rate > 0 && rate <= MAX_SAMPLE_RATE,
            Self::Exactly(rates) => rates.contains(&rate),
        }
    }
}

/// Whether one frame's output has exactly its input's position count (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LengthPolicy {
    /// Every accepted frame produces exactly as many positions as it consumed. Latency and tail are
    /// declarations about *content*, not about count: a delay line under this policy still writes
    /// one position out for one position in, and what is delayed is which input each output carries.
    Preserving,
    /// The processor may produce fewer or more positions than it consumed, never above this ceiling.
    Bounded {
        /// The most positions one frame may produce, whatever it consumed.
        max_positions: u32,
    },
}

/// Whether a reset is observable at all (§8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResetBehavior {
    /// The processor holds no sample memory, so a reset discards nothing.
    Stateless,
    /// A reset discards every retained position and every derived state. Declared parameters
    /// survive.
    ClearsState,
}

/// Everything a processor declares before it may be prepared or attached (§5).
///
/// A capability is read by the attachment to allocate every queue, sink and scratch region a call
/// will ever use, so a capability that lies about its shape is a capability that mis-sizes memory.
/// [`Self::validate`] catches that at the declaration and not at the first frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DspCapability {
    id: &'static str,
    rates: RateSupport,
    channels: &'static [u8],
    max_frame_samples: u32,
    scratch_samples: u32,
    state_bytes: u64,
    latency_positions: u32,
    tail_positions: u32,
    length: LengthPolicy,
    reset: ResetBehavior,
    execution: ExecutionPolicy,
    parameters: &'static [ParameterSpec],
}

impl DspCapability {
    /// The default declaration under `id`: any rate, mono, the full frame ceiling, no scratch, a
    /// 4,096-byte state bound, no latency, no tail, length preserving, state-clearing resets, the
    /// proven-inline profile's minimum policy and no parameters.
    ///
    /// Every `with_*` method changes one field. Nothing is validated here: a declaration is data,
    /// and [`Self::validate`] checks it once, before anything is sized by it.
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self {
            id,
            rates: RateSupport::Any,
            channels: &[1],
            max_frame_samples: MAX_FRAME_SAMPLES,
            scratch_samples: 0,
            state_bytes: 4_096,
            latency_positions: 0,
            tail_positions: 0,
            length: LengthPolicy::Preserving,
            reset: ResetBehavior::ClearsState,
            execution: ExecutionPolicy::new(ExecutionProfile::ProvenInline),
            parameters: &[],
        }
    }

    /// Rename the declaration. Useful where one processor wraps another.
    #[must_use]
    pub const fn with_id(mut self, id: &'static str) -> Self {
        self.id = id;
        self
    }

    /// Which sample rates are accepted.
    #[must_use]
    pub const fn with_rates(mut self, rates: RateSupport) -> Self {
        self.rates = rates;
        self
    }

    /// Which channel counts are accepted, strictly ascending and inside `1..=`[`MAX_CHANNELS`].
    #[must_use]
    pub const fn with_channels(mut self, channels: &'static [u8]) -> Self {
        self.channels = channels;
        self
    }

    /// The largest interleaved frame accepted. Domain `1..=`[`MAX_FRAME_SAMPLES`].
    #[must_use]
    pub const fn with_max_frame_samples(mut self, samples: u32) -> Self {
        self.max_frame_samples = samples;
        self
    }

    /// The caller-owned scratch needed, in `i16`s. The caller supplies exactly this much.
    #[must_use]
    pub const fn with_scratch_samples(mut self, samples: u32) -> Self {
        self.scratch_samples = samples;
        self
    }

    /// An upper bound on the memory the processor itself owns (§9.1): what it holds inline plus
    /// what it owns on the heap.
    ///
    /// Both halves are checked. `DSP-K9` holds the inline half against this figure on every run,
    /// and the heap half on a run given a
    /// [`HeapMeter`](crate::dsp::HeapMeter) — see that trait for why one cannot be built inside this
    /// workspace and where the one that measures these processors lives.
    #[must_use]
    pub const fn with_state_bytes(mut self, bytes: u64) -> Self {
        self.state_bytes = bytes;
        self
    }

    /// The algorithmic delay: the position offset between an input and the output carrying it.
    #[must_use]
    pub const fn with_latency_positions(mut self, positions: u32) -> Self {
        self.latency_positions = positions;
        self
    }

    /// How many positions remain retained when input stops, and `flush` may produce.
    #[must_use]
    pub const fn with_tail_positions(mut self, positions: u32) -> Self {
        self.tail_positions = positions;
        self
    }

    /// Whether one frame's output has exactly its input's position count.
    #[must_use]
    pub const fn with_length(mut self, length: LengthPolicy) -> Self {
        self.length = length;
        self
    }

    /// Whether a reset is observable at all.
    #[must_use]
    pub const fn with_reset(mut self, reset: ResetBehavior) -> Self {
        self.reset = reset;
        self
    }

    /// Where this processor may run and what happens when it is late.
    #[must_use]
    pub const fn with_execution(mut self, execution: ExecutionPolicy) -> Self {
        self.execution = execution;
        self
    }

    /// The closed parameter schema, with unique identifiers.
    #[must_use]
    pub const fn with_parameters(mut self, parameters: &'static [ParameterSpec]) -> Self {
        self.parameters = parameters;
        self
    }

    /// The stable identifier the SDK selects this processor by.
    #[must_use]
    pub const fn id(&self) -> &'static str {
        self.id
    }

    /// Which sample rates are accepted.
    #[must_use]
    pub const fn rates(&self) -> RateSupport {
        self.rates
    }

    /// Which channel counts are accepted.
    #[must_use]
    pub const fn channels(&self) -> &'static [u8] {
        self.channels
    }

    /// The largest interleaved frame accepted.
    #[must_use]
    pub const fn max_frame_samples(&self) -> u32 {
        self.max_frame_samples
    }

    /// The caller-owned scratch this processor is entitled to, in `i16`s.
    #[must_use]
    pub const fn scratch_samples(&self) -> u32 {
        self.scratch_samples
    }

    /// The declared upper bound on the memory the processor itself owns.
    #[must_use]
    pub const fn state_bytes(&self) -> u64 {
        self.state_bytes
    }

    /// The declared algorithmic delay, in positions.
    #[must_use]
    pub const fn latency_positions(&self) -> u32 {
        self.latency_positions
    }

    /// The declared tail, in positions.
    #[must_use]
    pub const fn tail_positions(&self) -> u32 {
        self.tail_positions
    }

    /// The declared length policy.
    #[must_use]
    pub const fn length(&self) -> LengthPolicy {
        self.length
    }

    /// The declared reset behaviour.
    #[must_use]
    pub const fn reset(&self) -> ResetBehavior {
        self.reset
    }

    /// The declared execution and failure policy.
    #[must_use]
    pub const fn execution(&self) -> ExecutionPolicy {
        self.execution
    }

    /// The declared parameter schema.
    #[must_use]
    pub const fn parameters(&self) -> &'static [ParameterSpec] {
        self.parameters
    }

    /// How many positions a sink must hold for a frame of `input_positions`.
    ///
    /// This is the only sanctioned way to size a sink, so that a processor cannot get more output
    /// room by being asked politely.
    #[must_use]
    pub const fn max_output_positions(&self, input_positions: u32) -> u32 {
        match self.length {
            LengthPolicy::Preserving => input_positions,
            LengthPolicy::Bounded { max_positions } => max_positions,
        }
    }

    /// Whether this declaration is internally admissible.
    ///
    /// # Errors
    ///
    /// A [`CapabilityError`] naming the first field §5 does not admit — an empty rate or channel
    /// list, a channel above [`MAX_CHANNELS`], a frame above [`MAX_FRAME_SAMPLES`], a duplicate
    /// parameter identifier, an inverted domain, a deadline of zero frames.
    pub fn validate(&self) -> Result<(), CapabilityError> {
        if self.id.is_empty() {
            return Err(CapabilityError::EmptyIdentifier);
        }
        if let RateSupport::Exactly(rates) = self.rates {
            if rates.is_empty() {
                return Err(CapabilityError::EmptyList { field: "rates" });
            }
            if !ascending_u32(rates) {
                return Err(CapabilityError::NotAscending { field: "rates" });
            }
            for rate in rates {
                if *rate == 0 || *rate > MAX_SAMPLE_RATE {
                    return Err(CapabilityError::Field {
                        field: "rates",
                        value: i64::from(*rate),
                    });
                }
            }
        }
        if self.channels.is_empty() {
            return Err(CapabilityError::EmptyList { field: "channels" });
        }
        if !ascending_u8(self.channels) {
            return Err(CapabilityError::NotAscending { field: "channels" });
        }
        for channels in self.channels {
            if *channels == 0 || *channels > MAX_CHANNELS {
                return Err(CapabilityError::Field {
                    field: "channels",
                    value: i64::from(*channels),
                });
            }
        }
        range(
            "max_frame_samples",
            self.max_frame_samples,
            1,
            MAX_FRAME_SAMPLES,
        )?;
        range(
            "scratch_samples",
            self.scratch_samples,
            0,
            MAX_SCRATCH_SAMPLES,
        )?;
        range(
            "latency_positions",
            self.latency_positions,
            0,
            MAX_LATENCY_POSITIONS,
        )?;
        range(
            "tail_positions",
            self.tail_positions,
            0,
            MAX_LATENCY_POSITIONS,
        )?;
        if let LengthPolicy::Bounded { max_positions } = self.length {
            range("max_positions", max_positions, 1, MAX_FRAME_SAMPLES)?;
        }
        range(
            "deadline_frames",
            self.execution.deadline_frames,
            1,
            MAX_DEADLINE_FRAMES,
        )?;
        range(
            "max_consecutive_misses",
            self.execution.max_consecutive_misses,
            1,
            MAX_CONSECUTIVE_MISSES,
        )?;
        if self.parameters.len() > MAX_PARAMETERS {
            return Err(CapabilityError::Field {
                field: "parameters",
                value: i64::try_from(self.parameters.len()).unwrap_or(i64::MAX),
            });
        }
        for (index, spec) in self.parameters.iter().enumerate() {
            if spec.id.is_empty() {
                return Err(CapabilityError::EmptyIdentifier);
            }
            if self
                .parameters
                .iter()
                .take(index)
                .any(|earlier| earlier.id == spec.id)
            {
                return Err(CapabilityError::DuplicateParameter { id: spec.id });
            }
            let inverted = match spec.domain {
                ParameterDomain::Flag => false,
                ParameterDomain::Integer { min, max } => min > max,
                ParameterDomain::Ratio { min, max } => min > max,
            };
            if inverted {
                return Err(CapabilityError::ParameterDomain { id: spec.id });
            }
        }
        Ok(())
    }

    /// Whether this processor accepts a stream format.
    ///
    /// # Errors
    ///
    /// [`FormatError::UnsupportedRate`] or [`FormatError::UnsupportedChannelCount`] outside the
    /// contract's own domains, and [`FormatError::RateNotAccepted`] or
    /// [`FormatError::ChannelsNotAccepted`] outside this declaration's.
    pub fn accepts_format(&self, format: StreamFormat) -> Result<(), FormatError> {
        StreamFormat::new(format.sample_rate, format.channels)?;
        if !self.rates.accepts(format.sample_rate) {
            return Err(FormatError::RateNotAccepted {
                rate: format.sample_rate,
            });
        }
        if !self.channels.contains(&format.channels) {
            return Err(FormatError::ChannelsNotAccepted {
                channels: format.channels,
            });
        }
        Ok(())
    }

    /// Whether a whole parameter set is admissible under this declaration.
    ///
    /// The set is validated entire before a caller assigns anything, which is what makes §3.6's
    /// "a set is applied at one position boundary or not at all" implementable rather than
    /// aspirational.
    ///
    /// # Errors
    ///
    /// [`ParameterError::TooMany`] past [`MAX_PARAMETERS`], [`ParameterError::Unknown`] for an
    /// identifier this processor does not declare, and [`ParameterSpec::accepts`]'s refusals.
    pub fn validate_parameters(&self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        if parameters.len() > MAX_PARAMETERS {
            return Err(ParameterError::TooMany {
                count: parameters.len(),
            });
        }
        for parameter in parameters {
            let Some(spec) = self.parameters.iter().find(|spec| spec.id == parameter.id) else {
                return Err(ParameterError::Unknown { id: parameter.id });
            };
            spec.accepts(parameter.value)?;
        }
        Ok(())
    }
}

fn ascending_u32(values: &[u32]) -> bool {
    values.windows(2).all(|pair| match pair {
        [left, right] => left < right,
        _ => true,
    })
}

fn ascending_u8(values: &[u8]) -> bool {
    values.windows(2).all(|pair| match pair {
        [left, right] => left < right,
        _ => true,
    })
}

fn range(field: &'static str, value: u32, low: u32, high: u32) -> Result<(), CapabilityError> {
    if value < low || value > high {
        return Err(CapabilityError::Field {
            field,
            value: i64::from(value),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------- errors ----

/// A declaration §5 does not admit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CapabilityError {
    /// The field's value is outside the domain §5 states for it.
    #[error("capability field `{field}` is {value}, which is outside its domain")]
    Field {
        /// The field, spelled as §5 spells it.
        field: &'static str,
        /// What was declared.
        value: i64,
    },
    /// A list §5 requires to be non-empty is empty.
    #[error("capability field `{field}` is empty, and §5 admits no empty list")]
    EmptyList {
        /// The field, spelled as §5 spells it.
        field: &'static str,
    },
    /// A list §5 requires to be strictly ascending is not.
    #[error("capability field `{field}` is not strictly ascending")]
    NotAscending {
        /// The field, spelled as §5 spells it.
        field: &'static str,
    },
    /// Two parameters share an identifier, so one of them can never be addressed.
    #[error("capability declares the parameter `{id}` twice")]
    DuplicateParameter {
        /// The repeated identifier.
        id: &'static str,
    },
    /// A processor or parameter identifier is empty.
    #[error("capability declares an empty identifier")]
    EmptyIdentifier,
    /// A parameter's declared range is inverted, so no value could satisfy it.
    #[error("capability declares an inverted range for the parameter `{id}`")]
    ParameterDomain {
        /// The parameter whose range is inverted.
        id: &'static str,
    },
}

/// Why a processor refused a stream format (§3.2, §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FormatError {
    /// The rate is outside the linear-PCM boundary's `1..=384,000` Hz domain.
    ///
    /// The inner error is [`PcmError`], reused rather than re-minted, so rate 0 and rate 384,001
    /// refuse with exactly the type that boundary's own PCM-4 vector names.
    #[error(transparent)]
    UnsupportedRate(#[from] PcmError),
    /// The channel count is outside `1..=`[`MAX_CHANNELS`].
    #[error("a stream of {channels} channels is outside 1..={MAX_CHANNELS}")]
    UnsupportedChannelCount {
        /// What was requested.
        channels: u8,
    },
    /// The rate is admissible but this processor did not declare it.
    #[error("this processor does not accept {rate} Hz")]
    RateNotAccepted {
        /// What was requested.
        rate: u32,
    },
    /// The channel count is admissible but this processor did not declare it.
    #[error("this processor does not accept {channels} channels")]
    ChannelsNotAccepted {
        /// What was requested.
        channels: u8,
    },
    /// The processor was cancelled, which is terminal (§8.4).
    #[error("this processor was cancelled, and cancellation is terminal")]
    Cancelled,
}

/// Why a processor refused a parameter set (§3.6).
///
/// A refusal rejects the whole set: the previous parameter state stays in force, untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParameterError {
    /// The processor declares no parameter under this identifier.
    #[error("this processor declares no parameter `{id}`")]
    Unknown {
        /// The identifier offered.
        id: &'static str,
    },
    /// The value is a different kind than the declared domain.
    #[error("the parameter `{id}` was given a value of the wrong kind")]
    KindMismatch {
        /// The parameter.
        id: &'static str,
    },
    /// The value is the right kind and outside the declared range.
    #[error("the parameter `{id}` was given a value outside its declared range")]
    OutOfRange {
        /// The parameter.
        id: &'static str,
    },
    /// More parameters than [`MAX_PARAMETERS`] were offered at once.
    #[error("{count} parameters were offered at once, and §5 admits at most {MAX_PARAMETERS}")]
    TooMany {
        /// How many were offered.
        count: usize,
    },
}

/// Why a processor refused a frame, or could not complete one (§8.3).
///
/// A refusal rejects the input and changes nothing: not the position expectation, not the epoch,
/// not one sample of state, and it writes nothing to the sink and enqueues no observation. A caller
/// that fixes its input and retries continues exactly where the stream stood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProcessError {
    /// A frame arrived before `prepare` declared a direction and format.
    #[error("a frame arrived before this processor was prepared")]
    NotPrepared,
    /// A frame or flush arrived after `cancel`, which is terminal (§8.4).
    #[error("this processor was cancelled, and cancellation is terminal")]
    Cancelled,
    /// The frame carries no samples, more than the declared ceiling, or a sample count that is not
    /// a multiple of the channel count.
    #[error("a frame carries no samples, too many of them, or a partial position")]
    MalformedFrame,
    /// The frame's direction is not the prepared one.
    #[error("a frame carries the direction this processor is not prepared for")]
    DirectionMismatch,
    /// The frame's format is not the prepared one. A rate change is a `prepare`, never a frame.
    #[error("a frame carries a format this processor is not prepared for")]
    FormatMismatch,
    /// The frame's position violates §3.5.
    #[error("a frame's position is not the contiguous one §3.5 requires")]
    PositionNotContiguous,
    /// A write past the sink's capacity. The already-written prefix is untouched.
    #[error("a processor wrote past its output sink")]
    OutputOverflow,
    /// A scratch request past the declared bound. The caller sizes scratch from the declaration, so
    /// this is a processor asking for memory it said it did not need.
    #[error("a processor asked for {requested} scratch samples against {available} declared")]
    ScratchExhausted {
        /// How much was asked for.
        requested: u32,
        /// How much the declaration entitles it to.
        available: u32,
    },
    /// The output's position count violates the declared length policy.
    #[error("a processor consumed {consumed} positions and produced {produced}")]
    LengthPolicyViolated {
        /// Positions in.
        consumed: u64,
        /// Positions out.
        produced: u64,
    },
    /// The processor refused otherwise valid input for its own stated reason.
    #[error("a processor refused a frame: {reason}")]
    Rejected {
        /// Why, in the processor's own words.
        reason: &'static str,
    },
}

// ------------------------------------------------------------------------- workspace ----

/// The caller-owned scratch a processor may use within one `process` call (§9.1).
///
/// The buffer is exactly the length the capability declared, so a processor cannot obtain more
/// working memory than it said it needed, whatever it does. Contents on entry are **unspecified**:
/// §4.7 requires a processor to write a region before reading it, and the conformance harness fills
/// this with a run-specific pattern so that a processor which reads unwritten scratch fails
/// determinism rather than passing on a machine where the leftovers happened to match.
#[derive(Debug)]
pub struct Scratch<'a> {
    buffer: &'a mut [i16],
    requested: u32,
}

impl<'a> Scratch<'a> {
    /// Lend a buffer for the duration of one `process` call.
    #[must_use]
    pub fn new(buffer: &'a mut [i16]) -> Self {
        Self {
            buffer,
            requested: 0,
        }
    }

    /// How many samples the declaration entitles this processor to.
    #[must_use]
    pub fn capacity(&self) -> u32 {
        u32::try_from(self.buffer.len()).unwrap_or(u32::MAX)
    }

    /// The largest request seen, **including refused ones**.
    ///
    /// A refused request is still a request, and the declaration is what a processor is held to:
    /// this is what lets `DSP-K9` catch a processor that asks for more than it declared and then
    /// swallows the refusal.
    #[must_use]
    pub fn requested(&self) -> u32 {
        self.requested
    }

    /// Borrow the first `samples` of the region.
    ///
    /// A processor needing two disjoint regions takes one of their combined length and splits it.
    ///
    /// # Errors
    ///
    /// [`ProcessError::ScratchExhausted`] past the declared bound.
    pub fn take(&mut self, samples: u32) -> Result<&mut [i16], ProcessError> {
        self.requested = self.requested.max(samples);
        let available = u32::try_from(self.buffer.len()).unwrap_or(u32::MAX);
        let wanted = usize::try_from(samples).unwrap_or(usize::MAX);
        match self.buffer.get_mut(..wanted) {
            Some(region) => Ok(region),
            None => Err(ProcessError::ScratchExhausted {
                requested: samples,
                available,
            }),
        }
    }
}

/// Where a processor writes its output and its observations (§6).
///
/// Both are caller-owned and neither grows. The sink refuses a write past its capacity with the
/// already-written prefix untouched; the observation queue coalesces at capacity into
/// [`DspObservation::Lost`] exactly as the analysis contract's queue does, so an undersized queue
/// is a visible counted fact rather than a silent absence.
#[derive(Debug)]
pub struct FrameSink<'a> {
    output: &'a mut [i16],
    written: usize,
    observations: &'a mut Vec<DspObservation>,
    observation_capacity: usize,
}

impl<'a> FrameSink<'a> {
    /// Lend an output buffer and an observation queue for the duration of one call.
    ///
    /// `output` is sized by [`DspCapability::max_output_positions`] times the channel count, and
    /// `observation_capacity` bounds the queue whatever `observations` was allocated with.
    #[must_use]
    pub fn new(
        output: &'a mut [i16],
        observations: &'a mut Vec<DspObservation>,
        observation_capacity: u32,
    ) -> Self {
        Self {
            output,
            written: 0,
            observations,
            observation_capacity: usize::try_from(observation_capacity).unwrap_or(usize::MAX),
        }
    }

    /// Append `samples`.
    ///
    /// # Errors
    ///
    /// [`ProcessError::OutputOverflow`] when they would not all fit. Nothing is written in that
    /// case: a partial write is how half a frame reaches a wire.
    pub fn write(&mut self, samples: &[i16]) -> Result<(), ProcessError> {
        let end = self.written.saturating_add(samples.len());
        match self.output.get_mut(self.written..end) {
            Some(region) => {
                region.copy_from_slice(samples);
                self.written = end;
                Ok(())
            }
            None => Err(ProcessError::OutputOverflow),
        }
    }

    /// Append one sample.
    ///
    /// # Errors
    ///
    /// [`ProcessError::OutputOverflow`] at capacity.
    pub fn push(&mut self, sample: i16) -> Result<(), ProcessError> {
        match self.output.get_mut(self.written) {
            Some(slot) => {
                *slot = sample;
                self.written = self.written.saturating_add(1);
                Ok(())
            }
            None => Err(ProcessError::OutputOverflow),
        }
    }

    /// How many samples the sink holds in total.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.output.len()
    }

    /// How many samples have been written.
    #[must_use]
    pub fn written(&self) -> usize {
        self.written
    }

    /// The written prefix.
    #[must_use]
    pub fn samples(&self) -> &[i16] {
        self.output.get(..self.written).unwrap_or(&[])
    }

    /// Enqueue one observation, coalescing at capacity rather than growing (§6).
    pub fn emit(&mut self, observation: DspObservation) {
        if self.observations.len() >= self.observation_capacity {
            match self.observations.last_mut() {
                // discard: §6. The newest retained entry becomes loss accounting rather than the
                // queue growing past the capacity the caller declared.
                Some(DspObservation::Lost { count }) => *count = count.saturating_add(1),
                Some(newest) => *newest = DspObservation::Lost { count: 2 },
                // A zero-capacity queue has nothing to coalesce into and nothing to lose.
                None => {}
            }
            return;
        }
        self.observations.push(observation);
    }
}

/// Why a processor restarted (§8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DspResetCause {
    /// The caller asked for it.
    Requested,
    /// A new format was declared.
    FormatChange,
    /// The seam declared a break in the timeline.
    Discontinuity {
        /// What the seam said happened.
        kind: DiscontinuityKind,
    },
}

/// One typed fact a processor produced about a frame (§6).
///
/// Deliberately small, and deliberately not a place for deadline misses, bypasses or terminal
/// failures: those are things the runtime observed about a processor, not things a processor
/// observed about a frame. Keeping them apart is what lets an intentional glitch effect and an
/// overload defect stay distinguishable in events and metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DspObservation {
    /// Output was clamped to full scale in this many positions.
    Saturated {
        /// How many positions clamped.
        positions: u32,
    },
    /// The processor produced its input unchanged for this frame.
    PassedThrough,
    /// A parameter transition completed at a position boundary.
    ParameterApplied {
        /// Which declared parameter.
        parameter: &'static str,
        /// Where in the current epoch it took effect.
        at_position: u64,
    },
    /// The processor restarted its own state.
    Restarted {
        /// Why.
        cause: DspResetCause,
    },
    /// Observations that had no queue slot (§6).
    ///
    /// Deterministic like everything else: the same input against the same capacity loses the same
    /// observations.
    Lost {
        /// How many observations this marker stands for.
        count: u32,
    },
}

// ------------------------------------------------------------------------- admission ----

/// What admitting a frame established (§3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Admitted {
    positions: u64,
    discontinuity: Option<DiscontinuityKind>,
}

impl Admitted {
    /// How many positions the frame carries.
    #[must_use]
    pub const fn positions(self) -> u64 {
        self.positions
    }

    /// The break the seam declared before this frame, if any.
    ///
    /// When this is `Some`, the processor MUST discard its sample memory before consuming the
    /// frame's own samples: those samples open the new epoch.
    #[must_use]
    pub const fn discontinuity(self) -> Option<DiscontinuityKind> {
        self.discontinuity
    }
}

/// The frame-admission state machine every processor shares (§3.5, §8.3).
///
/// A processor is not obliged to use this, but every refusal in §8.3 has to be implemented
/// identically by every processor or the contract is not one contract, and "typed refusal with no
/// partial state mutation" is exactly the obligation that is easy to get subtly wrong once per
/// implementation. [`Self::admit`] computes every check before it assigns anything, so a refusal
/// leaves the epoch, the position expectation and the caller's stream exactly where they stood.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameAdmission {
    prepared: Option<(AudioDirection, StreamFormat)>,
    next_position: Option<u64>,
    cancelled: bool,
}

impl FrameAdmission {
    /// An unprepared admission state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            prepared: None,
            next_position: None,
            cancelled: false,
        }
    }

    /// Declare the direction and format this processor will run on, opening a new epoch.
    ///
    /// # Errors
    ///
    /// [`FormatError::Cancelled`] after [`Self::cancel`], and the capability's own refusals for a
    /// format it does not accept. A refused format leaves the previous one in force, untouched: a
    /// malformed format change never half-applies.
    pub fn prepare(
        &mut self,
        capability: &DspCapability,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), FormatError> {
        if self.cancelled {
            return Err(FormatError::Cancelled);
        }
        capability.accepts_format(format)?;
        self.prepared = Some((direction, format));
        self.next_position = None;
        Ok(())
    }

    /// Admit one frame, or refuse it without changing anything.
    ///
    /// # Errors
    ///
    /// Every refusal in §8.3 that is a property of the frame rather than of the processor's own
    /// arithmetic.
    pub fn admit(
        &mut self,
        capability: &DspCapability,
        frame: &DspFrame<'_>,
    ) -> Result<Admitted, ProcessError> {
        if self.cancelled {
            return Err(ProcessError::Cancelled);
        }
        let Some((direction, format)) = self.prepared else {
            return Err(ProcessError::NotPrepared);
        };
        let Ok(samples) = u32::try_from(frame.samples().len()) else {
            return Err(ProcessError::MalformedFrame);
        };
        if samples == 0 || samples > MAX_FRAME_SAMPLES || samples > capability.max_frame_samples() {
            return Err(ProcessError::MalformedFrame);
        }
        if frame.direction() != direction {
            return Err(ProcessError::DirectionMismatch);
        }
        if frame.format() != format {
            return Err(ProcessError::FormatMismatch);
        }
        let channels = u32::from(format.channels());
        if samples % channels != 0 {
            return Err(ProcessError::MalformedFrame);
        }
        let positions = u64::from(samples / channels);

        // §3.5's table, evaluated in full before one field is assigned.
        let discontinuity = frame.discontinuity();
        let contiguous = match (self.next_position, discontinuity) {
            // A `Realign` restarts the epoch, which is what the seam already says it does, and
            // the first frame of an epoch opens that epoch at 0 for the same reason.
            (_, Some(DiscontinuityKind::Realign)) | (None, _) => frame.position() == 0,
            // The seam always flags a gap, so an unflagged one is a broken caller.
            (Some(expected), None) => frame.position() == expected,
            // A flagged gap may skip forward but never backward.
            (Some(expected), Some(_)) => frame.position() >= expected,
        };
        if !contiguous {
            return Err(ProcessError::PositionNotContiguous);
        }

        self.next_position = Some(frame.position().saturating_add(positions));
        Ok(Admitted {
            positions,
            discontinuity,
        })
    }

    /// Admit a flush.
    ///
    /// # Errors
    ///
    /// [`ProcessError::NotPrepared`] and [`ProcessError::Cancelled`].
    pub fn admit_flush(&mut self) -> Result<(), ProcessError> {
        if self.cancelled {
            return Err(ProcessError::Cancelled);
        }
        if self.prepared.is_none() {
            return Err(ProcessError::NotPrepared);
        }
        Ok(())
    }

    /// Open a new epoch: the next frame must arrive at position 0.
    pub const fn reset(&mut self) {
        self.next_position = None;
    }

    /// Terminate this processor. Idempotent, and there is no way back (§8.4).
    pub const fn cancel(&mut self) {
        self.cancelled = true;
        self.next_position = None;
    }

    /// Whether [`Self::cancel`] has been called.
    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    /// The direction and format in force, if this processor has been prepared.
    #[must_use]
    pub const fn prepared(&self) -> Option<(AudioDirection, StreamFormat)> {
        self.prepared
    }

    /// The position the next unflagged frame must arrive at, if an epoch is open.
    #[must_use]
    pub const fn next_position(&self) -> Option<u64> {
        self.next_position
    }
}

// ----------------------------------------------------------------------------- trait ----

/// The one deterministic frame transform built-in effects, noise reduction and
/// application-supplied DSP all implement (§1).
///
/// There is no crate-private door: `sipx`'s own processors implement exactly this, which is the
/// only way the claim that built-in and external processors behave identically is a fact rather
/// than an intention.
///
/// **What an implementation promises.** No I/O, no clock read, no task spawn, no device discovery,
/// no random source and no shared mutable state (§4). Time is the sample position and rate it is
/// handed. Its output is a pure function of its capability, parameters, format, direction and
/// ordered inputs, identical on every architecture and under virtual or wall time alike. It
/// allocates nothing after `prepare`: output, observations and working memory are all lent by the
/// caller. Every refusal is typed and mutates nothing.
///
/// **What it does not promise.** Nothing here bounds *wall time*: see [`ExecutionProfile`] for
/// which placement may claim that over-budget work cannot stall RTP, and which explicitly may not.
pub trait FrameProcessor {
    /// Everything this processor declares before it may run (§5).
    ///
    /// Callers size every buffer from this, so it must be constant for the lifetime of an instance.
    fn capability(&self) -> DspCapability;

    /// Apply a whole parameter set, or refuse it entire.
    ///
    /// # Errors
    ///
    /// A [`ParameterError`] naming the offending parameter. A refused set leaves the previous
    /// parameter state in force, untouched (§3.6).
    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError>;

    /// Declare the direction and format this processor will run on, opening a new epoch.
    ///
    /// # Errors
    ///
    /// A [`FormatError`] for a format outside the declaration. A refused format leaves the previous
    /// one in force (§8.3).
    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), FormatError>;

    /// Transform one frame.
    ///
    /// Samples are borrowed and MUST NOT be retained after this returns (§3.3).
    ///
    /// # Errors
    ///
    /// A [`ProcessError`] for each refusal in §8.3. A refusal writes nothing and mutates nothing.
    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError>;

    /// Write the retained tail — at most `tail_positions` — and leave nothing retained (§8.2).
    ///
    /// Flush is not a reset: it produces the tail rather than discarding it, and which of the two
    /// an end of input deserves is the caller's decision.
    ///
    /// # Errors
    ///
    /// A [`ProcessError`], including [`ProcessError::Cancelled`] after cancellation.
    fn flush(&mut self, sink: &mut FrameSink<'_>) -> Result<(), ProcessError>;

    /// Discard all sample memory and open a new epoch (§8.1).
    ///
    /// Retained audio is **discarded, not flushed**: it belongs to an epoch that no longer exists.
    /// Declared parameters survive. After this, [`Self::retained`] is 0.
    fn reset(&mut self, cause: DspResetCause);

    /// Terminate this processor. Idempotent, and there is no way back (§8.4).
    ///
    /// After this, [`Self::retained`] is 0 and every `process` and `flush` returns
    /// [`ProcessError::Cancelled`] without writing. This is the processor's half of the graph
    /// teardown barrier: a graph proving it holds zero frames needs each processor to be able to
    /// say it holds none.
    fn cancel(&mut self);

    /// How many positions of audio this processor is currently holding.
    ///
    /// Zero after `prepare`, after any reset, after a flush and after cancellation.
    fn retained(&self) -> u32;
}

impl<P: FrameProcessor + ?Sized> FrameProcessor for Box<P> {
    fn capability(&self) -> DspCapability {
        (**self).capability()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        (**self).configure(parameters)
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), FormatError> {
        (**self).prepare(direction, format)
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        (**self).process(frame, scratch, sink)
    }

    fn flush(&mut self, sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        (**self).flush(sink)
    }

    fn reset(&mut self, cause: DspResetCause) {
        (**self).reset(cause);
    }

    fn cancel(&mut self) {
        (**self).cancel();
    }

    fn retained(&self) -> u32 {
        (**self).retained()
    }
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

    /// §3.2: the rate refusal is the linear-PCM boundary's own type, reused rather than re-minted.
    #[test]
    fn the_rate_refusal_is_the_shared_one() {
        assert_eq!(
            StreamFormat::new(0, 1),
            Err(FormatError::UnsupportedRate(
                PcmError::UnsupportedSampleRate(0)
            ))
        );
        assert_eq!(
            StreamFormat::new(384_001, 1),
            Err(FormatError::UnsupportedRate(
                PcmError::UnsupportedSampleRate(384_001)
            ))
        );
        assert!(StreamFormat::new(384_000, 8).is_ok());
        assert_eq!(
            StreamFormat::new(8_000, 9),
            Err(FormatError::UnsupportedChannelCount { channels: 9 })
        );
    }

    /// §3.3: positions are samples divided by channels, and a partial position never survives
    /// admission to be divided at all.
    #[test]
    fn positions_count_one_sample_per_channel() {
        let stereo = StreamFormat::new(8_000, 2).unwrap();
        let frame = DspFrame::new(AudioDirection::Inbound, stereo, 0, &[1, 2, 3, 4]);
        assert_eq!(frame.positions(), 2);
    }

    /// §3.5: a partial position is refused, and the refusal leaves the epoch where it stood.
    #[test]
    fn a_partial_position_is_refused_without_state_change() {
        let capability = DspCapability::new("t").with_channels(&[2]);
        let stereo = StreamFormat::new(8_000, 2).unwrap();
        let mut admission = FrameAdmission::new();
        admission
            .prepare(&capability, AudioDirection::Inbound, stereo)
            .unwrap();

        let odd = DspFrame::new(AudioDirection::Inbound, stereo, 0, &[1, 2, 3]);
        assert_eq!(
            admission.admit(&capability, &odd),
            Err(ProcessError::MalformedFrame)
        );
        assert_eq!(admission.next_position(), None, "the epoch never opened");

        let even = DspFrame::new(AudioDirection::Inbound, stereo, 0, &[1, 2, 3, 4]);
        assert_eq!(admission.admit(&capability, &even).unwrap().positions(), 2);
        assert_eq!(admission.next_position(), Some(2));
    }

    /// §5: `validate` refuses before anything is sized by the declaration.
    #[test]
    fn a_capability_is_validated_before_it_sizes_anything() {
        const INVERTED: &[ParameterSpec] = &[ParameterSpec::new(
            "a",
            ParameterDomain::Integer { min: 5, max: 1 },
        )];
        assert_eq!(
            DspCapability::new("").validate(),
            Err(CapabilityError::EmptyIdentifier)
        );
        assert_eq!(
            DspCapability::new("t")
                .with_rates(RateSupport::Exactly(&[16_000, 8_000]))
                .validate(),
            Err(CapabilityError::NotAscending { field: "rates" })
        );
        assert_eq!(
            DspCapability::new("t")
                .with_rates(RateSupport::Exactly(&[]))
                .validate(),
            Err(CapabilityError::EmptyList { field: "rates" })
        );
        assert_eq!(
            DspCapability::new("t").with_parameters(INVERTED).validate(),
            Err(CapabilityError::ParameterDomain { id: "a" })
        );
    }

    /// §7: the containment claim is a property of the profile and of nothing else.
    #[test]
    fn only_two_profiles_may_claim_containment() {
        for profile in [
            ExecutionProfile::ProvenInline,
            ExecutionProfile::SupervisedIsolated,
        ] {
            assert!(profile.contains_overrun(), "{profile:?}");
        }
        assert!(!ExecutionProfile::TrustedCooperativeNative.contains_overrun());
        // And no configuration reaches it.
        let policy = ExecutionPolicy::new(ExecutionProfile::TrustedCooperativeNative)
            .with_deadline_frames(1)
            .with_max_consecutive_misses(1)
            .with_on_failure(FailureAction::TerminateClosed);
        assert!(!policy.contains_overrun());
    }

    /// §6: the observation queue coalesces exactly as the analysis contract's does.
    #[test]
    fn the_observation_queue_coalesces_at_capacity() {
        let mut output = [0i16; 1];
        let mut observations = Vec::with_capacity(2);
        let mut sink = FrameSink::new(&mut output, &mut observations, 2);
        for _ in 0..5 {
            sink.emit(DspObservation::PassedThrough);
        }
        assert_eq!(
            observations,
            vec![
                DspObservation::PassedThrough,
                DspObservation::Lost { count: 4 }
            ]
        );
    }

    /// §6: a refused write leaves the already-written prefix alone rather than half-filling it.
    #[test]
    fn a_refused_write_is_total() {
        let mut output = [0i16; 3];
        let mut observations = Vec::with_capacity(2);
        let mut sink = FrameSink::new(&mut output, &mut observations, 2);
        sink.write(&[1, 2]).unwrap();
        assert_eq!(sink.write(&[3, 4]), Err(ProcessError::OutputOverflow));
        assert_eq!(sink.samples(), [1, 2]);
    }
}
