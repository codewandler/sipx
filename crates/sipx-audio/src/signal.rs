//! Deterministic signal metrics for live call audio: level, energy, clipping and silence.
//!
//! The normative contract is
//! [`docs/specs/call-audio-processing.md`](../../../../docs/specs/call-audio-processing.md)
//! (`M-57`); this module implements the part of it `M-59` owns — §4's arithmetic, §5's windows and
//! per-window facts, §6's silence timeout, §7's reset and refusal taxonomy, and §8's bounds. Where
//! this code and that document disagree, the document is right.
//!
//! **This is signal content, not network quality.** Loss, jitter, round-trip time and the MOS
//! estimate belong to `M-10`'s RTCP snapshot ([`sipx_media::MediaSession::quality`], reached
//! from a call), and §10 of the contract forbids duplicating them here. Nothing in this module
//! measures packet delivery: a call with no loss at all can be clipping, and a call losing a
//! quarter of its packets can be reporting a clean level for the audio that did arrive. The two
//! answer different questions and neither substitutes for the other.
//!
//! **It is also not a speech model.** No recognition, no tone or DTMF classification (RFC 4733
//! DTMF keeps its one typed path), no gain assumption, and — deliberately — not an ITU-T P.56
//! active speech level. The numbers below are raw integer window facts.
//!
//! # What makes it reproducible
//!
//! The processor is a pure state machine. It reads no clock, owns no task, touches no socket and
//! allocates nothing after construction. Every duration is converted **once**, at construction and
//! at each accepted format change, into a sample count by §4's exact formula
//! `samples(d_ms, rate) = (d_ms · rate + 999) div 1000`; after that only sample counts exist. Every
//! decision is two's-complement integer arithmetic at a stated width, with no floating point
//! anywhere in the path — §5.3's variance predicate is written division-free precisely so that no
//! platform's rounding can enter the answer. Two processors built from one [`SignalProfile`] and
//! fed one input therefore drain identically on every machine.
//!
//! The one derived quantity that is not a raw accumulator, [`SignalReport::rms`], is defined as
//! exact integer arithmetic for the same reason: `floor(sqrt(floor(energy / samples)))`, in signed
//! sample-amplitude units.
//!
//! # What it does not decide
//!
//! Voice activity. §5.3's per-window `active` fact is computed and counted here because it is one
//! of the five facts the contract defines over the same accumulators, and because the level
//! vectors pin it — but the start/end/hangover state machine of §6 that turns it into a transition
//! is `M-58`'s, and no such transition is derived in this crate.
//!
//! # Example
//!
//! ```
//! use sipx_audio::signal::{
//!     SignalDirection, SignalFrame, SignalObservation, SignalProcessor, SignalProfile,
//! };
//!
//! let mut processor = SignalProcessor::new(
//!     SignalProfile::new(SignalDirection::Inbound, 8_000).with_window_ms(20),
//! )?;
//!
//! // One 20 ms window of a stuck full-scale capture.
//! processor.process(SignalFrame::new(SignalDirection::Inbound, 0, &[32_767i16; 160]))?;
//!
//! for observation in processor.drain() {
//!     if let SignalObservation::Report(report) = observation {
//!         assert_eq!(report.peak, 32_767);
//!         assert_eq!(report.clipping_windows, 1);
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::collections::VecDeque;

use crate::pcm::{MAX_SAMPLE_RATE, PcmEncoding, PcmError, PcmFormat};

/// The largest frame the processor accepts, in samples (§3.3).
///
/// The ceiling is the per-call CPU bound rather than a buffer size: `process` is O(samples), so
/// bounding the frame bounds the work one call can ask for in one call. It is also what makes
/// §5.2's width proof hold — a window can never be longer than this.
pub const MAX_FRAME_SAMPLES: usize = 65_536;

/// The longest window the processor derives, in samples (§5.1).
pub const MAX_WINDOW_SAMPLES: u64 = 65_536;

/// The most windows one report may coalesce.
pub const MAX_WINDOWS_PER_REPORT: u32 = 4_096;

/// The shallowest observation queue the processor accepts (§5.1).
pub const MIN_QUEUE_CAPACITY: u32 = 2;

/// The deepest observation queue the processor accepts (§5.1).
pub const MAX_QUEUE_CAPACITY: u32 = 4_096;

/// Which side of a call a processor observes (§3.1).
///
/// A processor is bound to exactly one direction at construction and refuses a frame tagged with
/// the other, so per-direction analysis state never interleaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalDirection {
    /// Audio decoded from the remote peer.
    Inbound,
    /// Audio produced locally for transmission.
    Outbound,
}

impl SignalDirection {
    /// Its lowercase spelling, shared with the call-media seam's own vocabulary.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inbound => "inbound",
            Self::Outbound => "outbound",
        }
    }
}

impl std::fmt::Display for SignalDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a frame's sample timeline broke (§3.3).
///
/// The vocabulary is the call-media seam's, not a second one: a discontinuity is what the seam
/// says happened, never what this processor infers from the samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SignalDiscontinuity {
    /// Upstream frames were lost, in the network or in decode.
    Loss,
    /// The seam dropped frames under its loss policy.
    Overflow,
    /// The seam re-anchored the timeline.
    Realign,
}

impl SignalDiscontinuity {
    /// Its lowercase spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Loss => "loss",
            Self::Overflow => "overflow",
            Self::Realign => "realign",
        }
    }
}

impl std::fmt::Display for SignalDiscontinuity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One frame of mono signed-16 samples with the metadata that places it on a timeline (§3.3).
///
/// Samples are **borrowed**. They are read during [`SignalProcessor::process`] and never retained:
/// raw call audio staying out of analysis state is a design invariant, not an optimization.
#[derive(Debug, Clone, Copy)]
pub struct SignalFrame<'a> {
    direction: SignalDirection,
    sequence: u64,
    discontinuity: Option<SignalDiscontinuity>,
    samples: &'a [i16],
}

impl<'a> SignalFrame<'a> {
    /// A frame with no discontinuity before it.
    #[must_use]
    pub const fn new(direction: SignalDirection, sequence: u64, samples: &'a [i16]) -> Self {
        Self {
            direction,
            sequence,
            discontinuity: None,
            samples,
        }
    }

    /// Declare the break immediately before this frame.
    ///
    /// The flag is authoritative: a flagged frame resets measurement (§7.1) before its own samples
    /// are consumed, so those samples open the new epoch.
    #[must_use]
    pub const fn with_discontinuity(mut self, kind: SignalDiscontinuity) -> Self {
        self.discontinuity = Some(kind);
        self
    }

    /// Which side of the call this audio belongs to.
    #[must_use]
    pub const fn direction(&self) -> SignalDirection {
        self.direction
    }

    /// The seam's frame number for this delivery.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// The break immediately before this frame, if there was one.
    #[must_use]
    pub const fn discontinuity(&self) -> Option<SignalDiscontinuity> {
        self.discontinuity
    }

    /// The borrowed samples.
    #[must_use]
    pub const fn samples(&self) -> &'a [i16] {
        self.samples
    }
}

/// What a processor measures and how often it says so (§5.1).
///
/// Durations are declared in milliseconds and converted to sample counts exactly once, against the
/// declared rate. Nothing is validated here: a profile is a request, and
/// [`SignalProcessor::new`] is where every domain is checked and named.
///
/// The contract's `hangover_ms` is deliberately absent. Hangover belongs to the voice-activity
/// transitions of §6, which are `M-58`'s half of the same contract; a field this processor would
/// never read would be a promise it does not keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalProfile {
    direction: SignalDirection,
    rate: u32,
    window_ms: u32,
    activation_amplitude: i32,
    silence_amplitude: i32,
    impulse_amplitude: i32,
    dc_amplitude: i32,
    clip_samples: u32,
    silence_timeout_ms: Option<u32>,
    windows_per_report: u32,
    queue_capacity: u32,
}

impl SignalProfile {
    /// A profile for one direction at one declared rate, with the contract's reference thresholds.
    ///
    /// The defaults are §11.1's reference profile `P8` apart from two deliberate choices:
    /// `windows_per_report` is 1, so a report is one window until a caller asks for a coarser
    /// cadence, and the silence timer is **off**. A processor that has not been told what counts
    /// as too much silence should not decide for the application; §5.1 makes that field optional
    /// for exactly this reason.
    #[must_use]
    pub const fn new(direction: SignalDirection, rate: u32) -> Self {
        Self {
            direction,
            rate,
            window_ms: 20,
            activation_amplitude: 2_048,
            silence_amplitude: 64,
            impulse_amplitude: 16_384,
            dc_amplitude: 512,
            clip_samples: 8,
            silence_timeout_ms: None,
            windows_per_report: 1,
            queue_capacity: 64,
        }
    }

    /// How long one measurement window is. Derived to `W` samples against the declared rate.
    #[must_use]
    pub const fn with_window_ms(mut self, milliseconds: u32) -> Self {
        self.window_ms = milliseconds;
        self
    }

    /// The amplitude a window's DC-free variation must reach to count as active (1..=32,767).
    #[must_use]
    pub const fn with_activation_amplitude(mut self, amplitude: i32) -> Self {
        self.activation_amplitude = amplitude;
        self
    }

    /// The floor nothing in a window may rise above for it to be silent (1..=32,768).
    #[must_use]
    pub const fn with_silence_amplitude(mut self, amplitude: i32) -> Self {
        self.silence_amplitude = amplitude;
        self
    }

    /// The peak a window must reach before it can be read as an impulse (1..=32,768).
    #[must_use]
    pub const fn with_impulse_amplitude(mut self, amplitude: i32) -> Self {
        self.impulse_amplitude = amplitude;
        self
    }

    /// The mean magnitude a window's offset must reach to be reported as DC (1..=32,767).
    #[must_use]
    pub const fn with_dc_amplitude(mut self, amplitude: i32) -> Self {
        self.dc_amplitude = amplitude;
        self
    }

    /// How many full-scale samples make a window clipping (1..=`W`).
    #[must_use]
    pub const fn with_clip_samples(mut self, samples: u32) -> Self {
        self.clip_samples = samples;
        self
    }

    /// How much unbroken silence elapses before it is reported, or `None` to disable the timer.
    #[must_use]
    pub const fn with_silence_timeout_ms(mut self, milliseconds: Option<u32>) -> Self {
        self.silence_timeout_ms = milliseconds;
        self
    }

    /// How many completed windows one [`SignalReport`] covers (1..=[`MAX_WINDOWS_PER_REPORT`]).
    ///
    /// This is the cadence bound: a call at 8,000 Hz with 20 ms windows completes fifty windows a
    /// second, and reporting each one separately would spend a call's whole event budget on
    /// arithmetic nobody asked for at that resolution. Coalescing happens in fixed-size state, so
    /// a coarser cadence costs no memory and loses no samples — only resolution.
    #[must_use]
    pub const fn with_windows_per_report(mut self, windows: u32) -> Self {
        self.windows_per_report = windows;
        self
    }

    /// How many observations the drain queue holds
    /// ([`MIN_QUEUE_CAPACITY`]..=[`MAX_QUEUE_CAPACITY`]).
    #[must_use]
    pub const fn with_queue_capacity(mut self, observations: u32) -> Self {
        self.queue_capacity = observations;
        self
    }

    /// The direction a processor built from this profile is bound to.
    #[must_use]
    pub const fn direction(self) -> SignalDirection {
        self.direction
    }

    /// The declared sample rate.
    #[must_use]
    pub const fn rate(self) -> u32 {
        self.rate
    }

    /// The declared window duration in milliseconds.
    #[must_use]
    pub const fn window_ms(self) -> u32 {
        self.window_ms
    }

    /// How many windows one report covers.
    #[must_use]
    pub const fn windows_per_report(self) -> u32 {
        self.windows_per_report
    }

    /// The declared silence timeout, if the timer is on.
    #[must_use]
    pub const fn silence_timeout_ms(self) -> Option<u32> {
        self.silence_timeout_ms
    }

    /// How many observations the drain queue holds.
    #[must_use]
    pub const fn queue_capacity(self) -> u32 {
        self.queue_capacity
    }
}

/// A configuration outside its declared domain (§5.1), named by the field that is wrong.
///
/// Every variant is returned before anything is allocated against the bad value, and — from
/// [`SignalProcessor::declare_format`] — before anything about the running measurement changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProfileError {
    /// The rate is outside the linear-PCM boundary's domain.
    ///
    /// The inner error is that boundary's own, reused rather than re-minted, so rate 0 and rate
    /// 384,001 refuse with exactly the type its `PCM-4` vector names.
    #[error("the analysis rate is outside the linear PCM domain: {0}")]
    UnsupportedSampleRate(#[from] PcmError),
    /// `window_ms` is zero, or the window it derives is outside 1..=[`MAX_WINDOW_SAMPLES`].
    ///
    /// Refused rather than clamped: a window silently shortened would compare against thresholds
    /// derived for a different length, which is a different, undeclared measurement.
    #[error(
        "window_ms {window_ms} derives {samples} samples at this rate; expected 1..={MAX_WINDOW_SAMPLES}"
    )]
    WindowMs {
        /// The declared duration.
        window_ms: u32,
        /// The window length it derived.
        samples: u64,
    },
    /// `activation_amplitude` is outside 1..=32,767.
    #[error("activation_amplitude {value} is outside 1..=32767")]
    ActivationAmplitude {
        /// What was asked for.
        value: i32,
    },
    /// `silence_amplitude` is outside 1..=32,768.
    #[error("silence_amplitude {value} is outside 1..=32768")]
    SilenceAmplitude {
        /// What was asked for.
        value: i32,
    },
    /// `impulse_amplitude` is outside 1..=32,768.
    #[error("impulse_amplitude {value} is outside 1..=32768")]
    ImpulseAmplitude {
        /// What was asked for.
        value: i32,
    },
    /// `dc_amplitude` is outside 1..=32,767.
    #[error("dc_amplitude {value} is outside 1..=32767")]
    DcAmplitude {
        /// What was asked for.
        value: i32,
    },
    /// `clip_samples` is zero or larger than the derived window.
    #[error("clip_samples {requested} is outside 1..={window}, the derived window")]
    ClipSamples {
        /// What was asked for.
        requested: u32,
        /// The window it has to fit in.
        window: u32,
    },
    /// `silence_timeout_ms` is `Some(0)`, or derives a count beyond `u32::MAX` samples.
    ///
    /// A zero timeout would fire before any silence existed.
    #[error("silence_timeout_ms {milliseconds} does not derive a usable sample count")]
    SilenceTimeoutMs {
        /// What was asked for.
        milliseconds: u32,
    },
    /// `windows_per_report` is outside 1..=[`MAX_WINDOWS_PER_REPORT`].
    #[error("windows_per_report {requested} is outside 1..={MAX_WINDOWS_PER_REPORT}")]
    WindowsPerReport {
        /// What was asked for.
        requested: u32,
    },
    /// `queue_capacity` is outside [`MIN_QUEUE_CAPACITY`]..=[`MAX_QUEUE_CAPACITY`].
    #[error("queue_capacity {requested} is outside {MIN_QUEUE_CAPACITY}..={MAX_QUEUE_CAPACITY}")]
    QueueCapacity {
        /// What was asked for.
        requested: u32,
    },
}

/// A frame the processor rejected (§7.3).
///
/// A refusal changes **nothing**: not the sequence expectation, not the stream position, not an
/// accumulator, and it enqueues no observation. A caller that fixes its input and retries
/// continues exactly where the stream stood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FrameError {
    /// The frame carries no samples, or more than [`MAX_FRAME_SAMPLES`] of them.
    ///
    /// Zero samples measure nothing, and a silent no-op there would hide a broken seam.
    #[error("a frame of {samples} samples is outside 1..={MAX_FRAME_SAMPLES}")]
    MalformedFrame {
        /// How many samples arrived.
        samples: usize,
    },
    /// The frame is tagged with the direction this processor is not bound to.
    #[error("this processor observes {expected} audio; the frame is {found}")]
    DirectionMismatch {
        /// The direction the processor is bound to.
        expected: SignalDirection,
        /// The direction the frame declared.
        found: SignalDirection,
    },
    /// The frame's sequence violates §3.4: it repeats or goes backwards, or it skips a number
    /// without the seam flagging the gap.
    ///
    /// The seam always flags a gap, so an unflagged one is a broken upstream rather than a loss to
    /// smooth over.
    #[error("frame sequence {found} does not follow {previous}")]
    MalformedSequence {
        /// The last accepted sequence.
        previous: u64,
        /// What arrived.
        found: u64,
    },
}

/// Why measurement restarted (§7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResetCause {
    /// The caller asked, through [`SignalProcessor::reset`].
    Requested,
    /// The declared format changed, through [`SignalProcessor::declare_format`].
    FormatChange {
        /// The rate now in force.
        rate: u32,
    },
    /// The seam declared a break in the sample timeline.
    Discontinuity {
        /// What the seam said happened.
        kind: SignalDiscontinuity,
    },
}

/// Signal facts over a run of consecutive completed windows.
///
/// Every field is exact integer arithmetic over the samples the run covered — there is no
/// smoothing, no gain and no estimate. `peak`, `rms` and the amplitude thresholds behind the
/// window counts are all in the same signed 16-bit sample-amplitude units, where 32,768 is the
/// magnitude of the most negative representable sample.
///
/// [`Self::epoch`] and [`Self::rate`] are what make a report unmistakably about *this* run of
/// *this* call at *this* format: both change on every reset, and a report is only ever built from
/// windows completed inside one epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SignalReport {
    /// The measurement run this report belongs to, counting from 0 and incremented by every reset.
    pub epoch: u64,
    /// The sample rate the coverage below is counted in.
    pub rate: u32,
    /// This report's number within its epoch, from 0, incrementing by 1. Restarts with the epoch.
    pub sequence: u64,
    /// The index of the first window covered, within the epoch.
    pub first_window: u64,
    /// How many consecutive windows are covered. Always the profile's `windows_per_report`.
    pub windows: u32,
    /// The position of the first sample covered, within the epoch.
    pub at_sample: u64,
    /// How many samples are covered: `windows · W`.
    pub samples: u64,
    /// The largest sample magnitude over the coverage, 0..=32,768.
    pub peak: i32,
    /// The sum of the samples — the coverage's DC content, signed.
    pub sum: i64,
    /// The sum of the squared samples.
    pub energy: i64,
    /// `floor(sqrt(floor(energy / samples)))`, in sample-amplitude units.
    ///
    /// Defined as exact integer arithmetic rather than a floating-point root so the same samples
    /// give the same level on every platform. It is a plain quadratic mean of the coverage and
    /// deliberately **not** an ITU-T P.56 active speech level: no speech gating, no gain
    /// assumption, and no door of this stack may describe it as a P.56 measurement.
    pub rms: u32,
    /// How many samples sat at full scale, positive or negative.
    pub clipped_samples: u64,
    /// How many covered windows reached the profile's `clip_samples` count.
    pub clipping_windows: u32,
    /// How many covered windows put more than half their energy in one sample: clicks, not signal.
    pub impulsive_windows: u32,
    /// How many covered windows met the activation threshold on DC-free variance.
    ///
    /// A count of §5.3's per-window fact, not a voice-activity decision. Start, end and hangover
    /// transitions are `M-58`'s and are not derived here.
    pub active_windows: u32,
    /// How many covered windows carried a mean magnitude at or above the DC threshold.
    pub dc_offset_windows: u32,
    /// How many covered windows stayed entirely below the silence floor.
    pub silent_windows: u32,
}

/// One fact the processor drained (§5, §6, §8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignalObservation {
    /// A completed reporting period.
    Report(SignalReport),
    /// Unbroken silence first reached the configured timeout.
    ///
    /// Emitted once per run of silent windows; it re-arms only after a non-silent window or a
    /// reset.
    SilenceElapsed {
        /// The position of the run's first sample, within the epoch.
        at_sample: u64,
        /// The epoch the run belongs to.
        epoch: u64,
    },
    /// Measurement restarted. The epoch named is the one this reset opened.
    Reset {
        /// Why.
        cause: ResetCause,
        /// The epoch now in force.
        epoch: u64,
    },
    /// Observations that had no room in the queue, counted rather than silently absent (§8.3).
    ///
    /// A caller that drains after every [`SignalProcessor::process`] never sees this. It exists so
    /// that an undersized queue or a consumer that stopped reading is a visible, deterministic
    /// fact instead of a gap nobody can measure.
    Lost {
        /// How many observations were coalesced into this marker.
        count: u64,
    },
}

/// The observations one [`SignalProcessor::drain`] yields, in order.
#[derive(Debug)]
pub struct Drain<'a> {
    inner: std::collections::vec_deque::Drain<'a, SignalObservation>,
}

impl Iterator for Drain<'_> {
    type Item = SignalObservation;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for Drain<'_> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

/// The five per-window facts of §5.3, computed from the window accumulators alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "these are the specification's five per-window facts, named as it names them; \
              folding them into a state machine would rename the contract"
)]
struct WindowFacts {
    clipping: bool,
    impulsive: bool,
    active: bool,
    dc_offset: bool,
    silent: bool,
}

/// The sample counts a profile derives against one declared rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Derived {
    rate: u32,
    /// `W`, in 1..=[`MAX_WINDOW_SAMPLES`].
    window: u32,
    /// The silence timeout as a sample count, when the timer is on.
    silence_timeout: Option<u64>,
}

/// §4's conversion, and the only division outside the level derivation:
/// `ceil(d_ms · rate / 1000)`, which the section also writes as `(d_ms · rate + 999) div 1000`.
///
/// The product cannot overflow: `u32::MAX · 384_000` is far inside `u64`.
fn samples_for(milliseconds: u32, rate: u32) -> u64 {
    (u64::from(milliseconds) * u64::from(rate)).div_ceil(1_000)
}

/// What one reporting period has accumulated so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Period {
    windows: u32,
    first_window: u64,
    at_sample: u64,
    peak: i32,
    sum: i64,
    energy: i64,
    clipped: u64,
    clipping: u32,
    impulsive: u32,
    active: u32,
    dc_offset: u32,
    silent: u32,
}

impl Period {
    const EMPTY: Self = Self {
        windows: 0,
        first_window: 0,
        at_sample: 0,
        peak: 0,
        sum: 0,
        energy: 0,
        clipped: 0,
        clipping: 0,
        impulsive: 0,
        active: 0,
        dc_offset: 0,
        silent: 0,
    };
}

/// A deterministic level, clipping and silence processor for one direction of one call.
///
/// See the [module documentation](self) for what it measures, what it refuses to measure, and why
/// its numbers are reproducible. Its complete input vocabulary is [`Self::process`],
/// [`Self::declare_format`] and [`Self::reset`]; its complete output vocabulary is what
/// [`Self::drain`] yields.
///
/// # Bounds
///
/// State is a constant of the configuration: the derived counts, one window's four accumulators,
/// the current period's twelve, the stream position, the silence run, the sequence base, and
/// `queue_capacity` preallocated observation slots. Nothing grows with call duration, frame count
/// or frame size, and after construction the processor performs no allocation.
#[derive(Debug)]
pub struct SignalProcessor {
    profile: SignalProfile,
    derived: Derived,

    /// Samples consumed into the window currently being filled, in 0..`W`.
    filled: u32,
    peak: i32,
    sum: i64,
    energy: i64,
    clipped: u32,

    period: Period,

    /// The next window index to complete, within the epoch.
    window_index: u64,
    epoch: u64,
    report_sequence: u64,
    /// The last accepted frame's sequence; `None` until the base is established (§3.4).
    previous_sequence: Option<u64>,

    /// Consecutive silent samples, and where that run started.
    silent_run: u64,
    silent_run_at: u64,
    silence_reported: bool,

    queue: VecDeque<SignalObservation>,
    /// The queue's bound, from the profile, resolved once so the hot path compares two `usize`s.
    capacity: usize,
}

impl SignalProcessor {
    /// Validate a profile and build the processor it describes.
    ///
    /// # Errors
    ///
    /// Returns the [`ProfileError`] naming the first field outside its domain (§5.1), before
    /// anything is allocated against it.
    pub fn new(profile: SignalProfile) -> Result<Self, ProfileError> {
        let derived = derive(&profile, profile.rate)?;
        // Proven by the domain `derive` just checked.
        let capacity = usize::try_from(profile.queue_capacity).unwrap_or(usize::MAX);
        Ok(Self {
            profile,
            derived,
            filled: 0,
            peak: 0,
            sum: 0,
            energy: 0,
            clipped: 0,
            period: Period::EMPTY,
            window_index: 0,
            epoch: 0,
            report_sequence: 0,
            previous_sequence: None,
            silent_run: 0,
            silent_run_at: 0,
            silence_reported: false,
            queue: VecDeque::with_capacity(capacity),
            capacity,
        })
    }

    /// The profile this processor was built from.
    #[must_use]
    pub const fn profile(&self) -> SignalProfile {
        self.profile
    }

    /// The rate currently in force, which a format change replaces.
    #[must_use]
    pub const fn rate(&self) -> u32 {
        self.derived.rate
    }

    /// The derived window length `W`, in samples at [`Self::rate`].
    #[must_use]
    pub const fn window_samples(&self) -> u32 {
        self.derived.window
    }

    /// The measurement run in force, counting from 0 and incremented by every reset.
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Consume one frame.
    ///
    /// A frame flagged with a discontinuity resets measurement (§7.1) before its own samples are
    /// consumed, so those samples open the new epoch.
    ///
    /// # Errors
    ///
    /// Returns a [`FrameError`] for an empty or oversized frame, the wrong direction, or a
    /// sequence that violates §3.4. A refusal leaves every part of the processor untouched.
    pub fn process(&mut self, frame: SignalFrame<'_>) -> Result<(), FrameError> {
        // Every refusal is decided before anything changes, so a caller that retries after fixing
        // its input continues exactly where the stream stood.
        if frame.direction != self.profile.direction {
            return Err(FrameError::DirectionMismatch {
                expected: self.profile.direction,
                found: frame.direction,
            });
        }
        let count = frame.samples.len();
        if count == 0 || count > MAX_FRAME_SAMPLES {
            return Err(FrameError::MalformedFrame { samples: count });
        }
        if let Some(previous) = self.previous_sequence {
            let ordered = frame.sequence > previous;
            let contiguous = frame.sequence == previous.wrapping_add(1);
            if !ordered || !(contiguous || frame.discontinuity.is_some()) {
                return Err(FrameError::MalformedSequence {
                    previous,
                    found: frame.sequence,
                });
            }
        }

        if let Some(kind) = frame.discontinuity {
            self.restart(ResetCause::Discontinuity { kind });
        }
        self.previous_sequence = Some(frame.sequence);

        for &sample in frame.samples {
            self.consume(sample);
        }
        Ok(())
    }

    /// Declare a new sample rate and restart measurement under it (§7.2).
    ///
    /// Every sample count is re-derived against the new rate, the §5.1 domains re-checked with it,
    /// and only then does anything change.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileError`] when the rate is outside the linear-PCM domain, or when the
    /// re-derivation leaves one. The previous format stays in force and nothing is observed: a
    /// malformed format change never half-applies.
    pub fn declare_format(&mut self, rate: u32) -> Result<(), ProfileError> {
        let derived = derive(&self.profile, rate)?;
        self.derived = derived;
        self.restart(ResetCause::FormatChange { rate });
        Ok(())
    }

    /// Restart measurement at the caller's request (§7.1).
    pub fn reset(&mut self) {
        self.restart(ResetCause::Requested);
    }

    /// Take every queued observation, in order, leaving the queue empty.
    pub fn drain(&mut self) -> Drain<'_> {
        Drain {
            inner: self.queue.drain(..),
        }
    }

    /// How many observations are waiting to be drained.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Clear every piece of runtime state and observe the reset.
    ///
    /// The configuration survives, and so does the queue and everything already in it: facts
    /// already earned are not destroyed by a reset. A partially filled window and a partially
    /// filled reporting period are discarded **unemitted** — a fact computed over fewer samples
    /// than its thresholds were derived for would be a different, undeclared measurement.
    fn restart(&mut self, cause: ResetCause) {
        self.epoch += 1;
        self.enqueue(SignalObservation::Reset {
            cause,
            epoch: self.epoch,
        });

        self.filled = 0;
        self.peak = 0;
        self.sum = 0;
        self.energy = 0;
        self.clipped = 0;
        self.period = Period::EMPTY;
        self.window_index = 0;
        self.report_sequence = 0;
        self.previous_sequence = None;
        self.silent_run = 0;
        self.silent_run_at = 0;
        self.silence_reported = false;
    }

    /// One sample into the window being filled: §8.2's fixed number of integer operations.
    fn consume(&mut self, sample: i16) {
        let magnitude = i32::from(sample.unsigned_abs());
        if magnitude > self.peak {
            self.peak = magnitude;
        }
        let value = i64::from(sample);
        self.sum += value;
        self.energy += value * value;
        if sample == i16::MAX || sample == i16::MIN {
            self.clipped += 1;
        }
        self.filled += 1;

        if self.filled == self.derived.window {
            self.complete_window();
        }
    }

    /// Close the current window, fold it into the period, and observe whatever that completed.
    ///
    /// Ordering is fixed (§6): the report that closes on this window first, then the silence
    /// transition it may have caused.
    fn complete_window(&mut self) {
        let facts = self.window_facts();
        let window = u64::from(self.derived.window);
        let at_sample = self.window_index * window;

        if self.period.windows == 0 {
            self.period.first_window = self.window_index;
            self.period.at_sample = at_sample;
        }
        self.period.windows += 1;
        if self.peak > self.period.peak {
            self.period.peak = self.peak;
        }
        self.period.sum += self.sum;
        self.period.energy += self.energy;
        self.period.clipped += u64::from(self.clipped);
        self.period.clipping += u32::from(facts.clipping);
        self.period.impulsive += u32::from(facts.impulsive);
        self.period.active += u32::from(facts.active);
        self.period.dc_offset += u32::from(facts.dc_offset);
        self.period.silent += u32::from(facts.silent);

        self.window_index += 1;
        self.filled = 0;
        self.peak = 0;
        self.sum = 0;
        self.energy = 0;
        self.clipped = 0;

        if self.period.windows == self.profile.windows_per_report {
            let report = self.close_period(window);
            self.enqueue(SignalObservation::Report(report));
        }

        self.track_silence(facts.silent, at_sample, window);
    }

    /// Build the report for a completed period and clear it.
    fn close_period(&mut self, window: u64) -> SignalReport {
        let period = self.period;
        self.period = Period::EMPTY;

        let samples = u64::from(period.windows) * window;
        // `energy` is a sum of squares and so never negative; `samples` is at least one window.
        let mean_square = u64::try_from(period.energy).unwrap_or(0) / samples;
        let rms = u32::try_from(mean_square.isqrt()).unwrap_or(u32::MAX);

        let report = SignalReport {
            epoch: self.epoch,
            rate: self.derived.rate,
            sequence: self.report_sequence,
            first_window: period.first_window,
            windows: period.windows,
            at_sample: period.at_sample,
            samples,
            peak: period.peak,
            sum: period.sum,
            energy: period.energy,
            rms,
            clipped_samples: period.clipped,
            clipping_windows: period.clipping,
            impulsive_windows: period.impulsive,
            active_windows: period.active,
            dc_offset_windows: period.dc_offset,
            silent_windows: period.silent,
        };
        self.report_sequence += 1;
        report
    }

    /// §6's silence timeout: independent of activity, fired once per run, re-armed by any
    /// non-silent window.
    fn track_silence(&mut self, silent: bool, at_sample: u64, window: u64) {
        if !silent {
            self.silent_run = 0;
            self.silence_reported = false;
            return;
        }
        if self.silent_run == 0 {
            self.silent_run_at = at_sample;
        }
        self.silent_run += window;

        let Some(timeout) = self.derived.silence_timeout else {
            return;
        };
        if !self.silence_reported && self.silent_run >= timeout {
            self.silence_reported = true;
            self.enqueue(SignalObservation::SilenceElapsed {
                at_sample: self.silent_run_at,
                epoch: self.epoch,
            });
        }
    }

    /// §5.3's five predicates, in the order the section states them.
    ///
    /// Every comparison is integer and every quantity fits `i64` with headroom: with
    /// `W <= 2^16` and `|s| <= 2^15`, `energy <= 2^46`, `W · energy <= 2^62`, `|sum| <= 2^31` so
    /// `sum² <= 2^62`, and `A² · W² <= 2^62`. The debug assertions below are §4's obligation to
    /// prove that rather than saturate silently.
    fn window_facts(&self) -> WindowFacts {
        let window = i64::from(self.derived.window);
        let peak = i64::from(self.peak);

        debug_assert!(self.energy <= 1i64 << 46, "energy width proof");
        debug_assert!(self.sum.abs() <= 1i64 << 31, "sum width proof");

        let clipping = self.clipped >= self.profile.clip_samples;
        let impulsive = self.peak >= self.profile.impulse_amplitude && self.energy < 2 * peak * peak;

        // `(W·energy − sum²)/W²` is exactly the window's variance, so this is `variance >= A²`
        // with no division performed and no rounding to disagree about. A constant signal — a
        // stuck DAC, a DC-biased capture — has variance exactly zero and is not activity, however
        // much energy it carries.
        let variation = window * self.energy - self.sum * self.sum;
        let activation = i64::from(self.profile.activation_amplitude);
        let active = !impulsive && variation >= activation * activation * window * window;

        let dc_offset = self.sum.abs() >= i64::from(self.profile.dc_amplitude) * window;
        let silent = self.peak < self.profile.silence_amplitude;

        WindowFacts {
            clipping,
            impulsive,
            active,
            dc_offset,
            silent,
        }
    }

    /// §8.3: the queue never blocks and never grows. When an enqueue would exceed capacity the
    /// newest retained entry is coalesced into loss accounting instead.
    fn enqueue(&mut self, observation: SignalObservation) {
        if self.queue.len() < self.capacity {
            self.queue.push_back(observation);
            return;
        }
        match self.queue.back_mut() {
            Some(SignalObservation::Lost { count }) => *count += 1,
            Some(newest) => *newest = SignalObservation::Lost { count: 2 },
            // Unreachable: the capacity domain starts at two, so a full queue has a newest entry.
            None => {}
        }
    }
}

/// Validate a profile against one rate and derive every sample count it needs (§4, §5.1).
fn derive(profile: &SignalProfile, rate: u32) -> Result<Derived, ProfileError> {
    // The rate refusal is the linear-PCM boundary's own, reused rather than re-minted.
    let format = PcmFormat::new(rate, PcmEncoding::Signed16)?;
    let rate = format.sample_rate();
    debug_assert!(rate <= MAX_SAMPLE_RATE, "the boundary bounds the rate");

    if profile.window_ms == 0 {
        return Err(ProfileError::WindowMs {
            window_ms: 0,
            samples: 0,
        });
    }
    let window = samples_for(profile.window_ms, rate);
    if window == 0 || window > MAX_WINDOW_SAMPLES {
        return Err(ProfileError::WindowMs {
            window_ms: profile.window_ms,
            samples: window,
        });
    }
    // Proven by the bound just checked.
    let window = u32::try_from(window).unwrap_or(u32::MAX);

    if !(1..=32_767).contains(&profile.activation_amplitude) {
        return Err(ProfileError::ActivationAmplitude {
            value: profile.activation_amplitude,
        });
    }
    if !(1..=32_768).contains(&profile.silence_amplitude) {
        return Err(ProfileError::SilenceAmplitude {
            value: profile.silence_amplitude,
        });
    }
    if !(1..=32_768).contains(&profile.impulse_amplitude) {
        return Err(ProfileError::ImpulseAmplitude {
            value: profile.impulse_amplitude,
        });
    }
    if !(1..=32_767).contains(&profile.dc_amplitude) {
        return Err(ProfileError::DcAmplitude {
            value: profile.dc_amplitude,
        });
    }
    if profile.clip_samples == 0 || profile.clip_samples > window {
        return Err(ProfileError::ClipSamples {
            requested: profile.clip_samples,
            window,
        });
    }
    if !(1..=MAX_WINDOWS_PER_REPORT).contains(&profile.windows_per_report) {
        return Err(ProfileError::WindowsPerReport {
            requested: profile.windows_per_report,
        });
    }
    if !(MIN_QUEUE_CAPACITY..=MAX_QUEUE_CAPACITY).contains(&profile.queue_capacity) {
        return Err(ProfileError::QueueCapacity {
            requested: profile.queue_capacity,
        });
    }

    let silence_timeout = match profile.silence_timeout_ms {
        None => None,
        Some(milliseconds) => {
            let count = samples_for(milliseconds, rate);
            if milliseconds == 0 || count == 0 || count > u64::from(u32::MAX) {
                return Err(ProfileError::SilenceTimeoutMs { milliseconds });
            }
            Some(count)
        }
    };

    Ok(Derived {
        rate,
        window,
        silence_timeout,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn reference() -> SignalProfile {
        SignalProfile::new(SignalDirection::Inbound, 8_000)
            .with_window_ms(20)
            .with_silence_timeout_ms(Some(2_000))
    }

    /// §4's conversion rounds up, so a window never covers less than the duration it names.
    #[test]
    fn the_duration_conversion_rounds_up() {
        assert_eq!(samples_for(20, 8_000), 160);
        assert_eq!(samples_for(20, 8_193), 164);
        assert_eq!(samples_for(20, 1), 1);
        assert_eq!(samples_for(2_000, 8_000), 16_000);
    }

    /// The queue coalesces rather than growing, however far behind the consumer is (§8.3).
    #[test]
    fn the_queue_never_grows_past_its_capacity() {
        let mut processor =
            SignalProcessor::new(reference().with_queue_capacity(2).with_clip_samples(1)).unwrap();

        for sequence in 0..64 {
            processor
                .process(SignalFrame::new(
                    SignalDirection::Inbound,
                    sequence,
                    &[0i16; 160],
                ))
                .unwrap();
            assert!(processor.queued() <= 2, "the queue is bounded at capacity");
        }

        let observations: Vec<_> = processor.drain().collect();
        assert_eq!(observations.len(), 2);
        assert!(matches!(
            observations[1],
            SignalObservation::Lost { count } if count > 0
        ));
    }

    /// A reset keeps the facts already earned: the queue and its contents survive it (§7.1).
    #[test]
    fn a_reset_does_not_destroy_observations_already_queued() {
        let mut processor = SignalProcessor::new(reference()).unwrap();
        processor
            .process(SignalFrame::new(SignalDirection::Inbound, 0, &[0i16; 160]))
            .unwrap();
        assert_eq!(processor.queued(), 1);

        processor.reset();
        let observations: Vec<_> = processor.drain().collect();
        assert!(matches!(observations[0], SignalObservation::Report(_)));
        assert!(matches!(
            observations[1],
            SignalObservation::Reset {
                cause: ResetCause::Requested,
                epoch: 1
            }
        ));
    }
}
