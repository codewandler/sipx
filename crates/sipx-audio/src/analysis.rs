//! Deterministic call-audio frame processing: the sans-I/O analyser of
//! [`docs/specs/call-audio-processing.md`](../../../docs/specs/call-audio-processing.md) (`M-57`).
//!
//! [`AudioAnalyzer`] is a pure state machine. Its whole input vocabulary is a validated
//! [`AnalysisProfile`], [`AnalysisFrame`]s carrying explicit metadata, declared format changes and
//! requested resets; its whole output vocabulary is typed [`Observation`]s the caller drains. It
//! owns no socket, no device, no clock, no thread and no task, and it reads no wall clock: every
//! duration is converted once, at construction and at each accepted format change, into a sample
//! count derived from the declared rate (§4). That is what makes the central promise hold —
//! **identical inputs produce identical observations on every machine** — so a fixture is evidence
//! and a regression is a diff.
//!
//! Delivering frames from a live call is not this module's job. That is `M-54`'s bounded PCM seam
//! ([`docs/specs/call-audio-seam.md`](../../../docs/specs/call-audio-seam.md)), of which an
//! analyser is one consumer; the spec forbids a second call-media tap by name (§9). Carrying
//! voice-activity transitions into an application's event stream is `sipx-call`'s (`M-58`).
//!
//! No speech model is loaded and none is reachable from here: voice activity is the integer
//! variance predicate of §5.3 over a fixed window, not recognition.
//!
//! ```
//! use sipx_audio::analysis::{AnalysisFrame, AnalysisProfile, AudioAnalyzer, AudioDirection, Observation};
//!
//! // §11.1's reference profile: 8,000 Hz, 20 ms windows, 200 ms hangover.
//! let mut analyzer = AudioAnalyzer::new(AnalysisProfile::new(AudioDirection::Inbound, 8_000))?;
//! let modulated: Vec<i16> = (0..160).map(|n| if n % 2 == 0 { 8_192 } else { -8_192 }).collect();
//! analyzer.process(&AnalysisFrame::new(AudioDirection::Inbound, 0, &modulated))?;
//!
//! assert!(analyzer.drain().any(|o| matches!(o, Observation::VoiceStarted { at_sample: 0 })));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::collections::VecDeque;
use std::collections::vec_deque::Drain;

use crate::pcm::{MAX_SAMPLE_RATE, PcmError};

/// The longest window §5.1 admits, in samples.
///
/// The cap is not a taste: §5.2's width proof — `energy <= W · 2^30`, `W · energy <= 2^62`,
/// `|sum| <= W · 2^15` — is what makes every comparison in §5.3 fit `i64`, and it needs
/// `W <= 2^16`. A 200 ms window at 384,000 Hz derives 76,800 and is refused rather than clamped.
pub const MAX_WINDOW_SAMPLES: u32 = 65_536;

/// The deepest observation queue §5.1 admits.
///
/// The domain is the seam's own (`docs/specs/call-audio-seam.md` §5), reused rather than re-minted.
pub const MAX_QUEUE_CAPACITY: u32 = 4_096;
/// The shallowest observation queue §5.1 admits.
pub const MIN_QUEUE_CAPACITY: u32 = 2;

/// The largest frame §3.3 admits, in samples.
///
/// The per-call CPU bound of §8.2: `process` is `O(samples)` with constant per-sample work, so a
/// frame ceiling is a hard ceiling on one call's analysis cost.
pub const MAX_FRAME_SAMPLES: usize = 65_536;

/// Which side of the call an analyser observes (§3.1).
///
/// Closed by design: a call has two audio directions, and a third would be a different contract
/// rather than a new variant of this one. An analyser is bound to exactly one direction at
/// construction and refuses the other's frames, which is what makes every vector in §11 a
/// single-stream replay.
///
/// This is also the seam's direction vocabulary — `sipx-media` re-exports this type rather than
/// minting a parallel one, because two observers of one call that name its sides differently are
/// how a disagreement about what happened starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioDirection {
    /// Audio decoded from the remote peer, tapped at the jitter buffer's output.
    Inbound,
    /// Audio produced locally for transmission, tapped after the mute gate and before encoding.
    Outbound,
}

impl std::fmt::Display for AudioDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inbound => f.write_str("inbound"),
            Self::Outbound => f.write_str("outbound"),
        }
    }
}

/// Why a sample timeline broke (§3.3).
///
/// The flag is authoritative: a discontinuity is what the seam says happened, never what an
/// analyser infers from a sequence gap. The vocabulary is shared with the seam and extended
/// compatibly, so a consumer writes a wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiscontinuityKind {
    /// Upstream frames were lost, in the network or in decode.
    Loss,
    /// The seam dropped frames under its bounded-queue loss policy.
    Overflow,
    /// The seam re-anchored the timeline.
    Realign,
}

impl std::fmt::Display for DiscontinuityKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loss => f.write_str("loss"),
            Self::Overflow => f.write_str("overflow"),
            Self::Realign => f.write_str("realign"),
        }
    }
}

/// A configured analyser (§5.1).
///
/// [`Self::new`] starts from §11.1's reference profile `P8` at the rate given — 20 ms windows, a
/// 2,048 activation amplitude, a 64 silence floor, a 16,384 impulse amplitude, a 512 DC amplitude,
/// 8 clipped samples, a 200 ms hangover, a 2 s silence timeout and a 64-observation queue — and the
/// `with_*` methods change one field each. Nothing is validated here: a profile is data, and every
/// domain in §5.1 is checked once, by [`AudioAnalyzer::new`], before anything is sized by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisProfile {
    direction: AudioDirection,
    rate: u32,
    window_ms: u32,
    activation_amplitude: i32,
    silence_amplitude: i32,
    impulse_amplitude: i32,
    dc_amplitude: i32,
    clip_samples: u32,
    hangover_ms: u32,
    silence_timeout_ms: Option<u32>,
    queue_capacity: u32,
}

impl AnalysisProfile {
    /// §11.1's reference profile at `rate`, observing `direction`.
    #[must_use]
    pub const fn new(direction: AudioDirection, rate: u32) -> Self {
        Self {
            direction,
            rate,
            window_ms: 20,
            activation_amplitude: 2_048,
            silence_amplitude: 64,
            impulse_amplitude: 16_384,
            dc_amplitude: 512,
            clip_samples: 8,
            hangover_ms: 200,
            silence_timeout_ms: Some(2_000),
            queue_capacity: 64,
        }
    }

    /// How long one measurement window is. Derives `W`; the domain is `1..=`[`MAX_WINDOW_SAMPLES`].
    #[must_use]
    pub const fn with_window_ms(mut self, window_ms: u32) -> Self {
        self.window_ms = window_ms;
        self
    }

    /// The window standard deviation at which audio counts as voice. Domain `1..=32,767`.
    #[must_use]
    pub const fn with_activation_amplitude(mut self, amplitude: i32) -> Self {
        self.activation_amplitude = amplitude;
        self
    }

    /// The peak below which a window is silent. Domain `1..=32,768`.
    #[must_use]
    pub const fn with_silence_amplitude(mut self, amplitude: i32) -> Self {
        self.silence_amplitude = amplitude;
        self
    }

    /// The peak at or above which a window may be a click rather than a signal. Domain `1..=32,768`.
    #[must_use]
    pub const fn with_impulse_amplitude(mut self, amplitude: i32) -> Self {
        self.impulse_amplitude = amplitude;
        self
    }

    /// The mean magnitude at or above which a window carries a DC offset. Domain `1..=32,767`.
    #[must_use]
    pub const fn with_dc_amplitude(mut self, amplitude: i32) -> Self {
        self.dc_amplitude = amplitude;
        self
    }

    /// How many full-scale samples make a window clipped. Domain `1..=W`.
    #[must_use]
    pub const fn with_clip_samples(mut self, samples: u32) -> Self {
        self.clip_samples = samples;
        self
    }

    /// How long voice survives inactive windows before it ends. Zero ends it at the first one.
    #[must_use]
    pub const fn with_hangover_ms(mut self, hangover_ms: u32) -> Self {
        self.hangover_ms = hangover_ms;
        self
    }

    /// How long an unbroken silent run runs before it is reported, or `None` to disable the timer.
    ///
    /// `Some(0)` is refused: a timeout of zero would fire before any silence existed.
    #[must_use]
    pub const fn with_silence_timeout_ms(mut self, timeout_ms: Option<u32>) -> Self {
        self.silence_timeout_ms = timeout_ms;
        self
    }

    /// How many observations the drain queue holds. Domain `2..=4,096`, the seam's own.
    #[must_use]
    pub const fn with_queue_capacity(mut self, observations: u32) -> Self {
        self.queue_capacity = observations;
        self
    }

    /// The direction this profile binds an analyser to.
    #[must_use]
    pub const fn direction(self) -> AudioDirection {
        self.direction
    }

    /// The declared sample rate every duration above is derived against.
    #[must_use]
    pub const fn rate(self) -> u32 {
        self.rate
    }

    /// The configured window duration.
    #[must_use]
    pub const fn window_ms(self) -> u32 {
        self.window_ms
    }

    /// The configured activation amplitude.
    #[must_use]
    pub const fn activation_amplitude(self) -> i32 {
        self.activation_amplitude
    }

    /// The configured silence floor.
    #[must_use]
    pub const fn silence_amplitude(self) -> i32 {
        self.silence_amplitude
    }

    /// The configured impulse amplitude.
    #[must_use]
    pub const fn impulse_amplitude(self) -> i32 {
        self.impulse_amplitude
    }

    /// The configured DC amplitude.
    #[must_use]
    pub const fn dc_amplitude(self) -> i32 {
        self.dc_amplitude
    }

    /// The configured clipped-sample count.
    #[must_use]
    pub const fn clip_samples(self) -> u32 {
        self.clip_samples
    }

    /// The configured hangover duration.
    #[must_use]
    pub const fn hangover_ms(self) -> u32 {
        self.hangover_ms
    }

    /// The configured silence timeout, if the timer is enabled.
    #[must_use]
    pub const fn silence_timeout_ms(self) -> Option<u32> {
        self.silence_timeout_ms
    }

    /// The configured observation-queue depth.
    #[must_use]
    pub const fn queue_capacity(self) -> u32 {
        self.queue_capacity
    }
}

/// A configured value §5.1 does not admit.
///
/// Every variant names the field it refuses, before any allocation is sized by the bad value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProfileError {
    /// The field's value is outside the domain §5.1 states for it.
    #[error("analysis profile field `{field}` is {value}, which is outside its domain")]
    Field {
        /// The field, spelled as §5.1 spells it.
        field: &'static str,
        /// What was configured.
        value: i64,
    },
    /// The window length derived from `window_ms` and the declared rate is outside `1..=65536`.
    ///
    /// Refused rather than clamped: a clamped window would measure a different duration than the
    /// one configured, and compare it against thresholds derived for the one that was asked for.
    #[error(
        "analysis profile field `window_ms` of {window_ms} ms at {rate} Hz derives {samples} \
         samples, which is outside 1..={MAX_WINDOW_SAMPLES}"
    )]
    WindowSamples {
        /// The configured window duration.
        window_ms: u32,
        /// The rate it was derived against.
        rate: u32,
        /// What the §4 formula produced.
        samples: u64,
    },
    /// A derived hangover or silence-timeout count does not fit the `u32` bound §5.1 states.
    #[error(
        "analysis profile field `{field}` derives {samples} samples at {rate} Hz, which is \
             outside 0..={}",
        u32::MAX
    )]
    DerivedCount {
        /// The field, spelled as §5.1 spells it.
        field: &'static str,
        /// The rate it was derived against.
        rate: u32,
        /// What the §4 formula produced.
        samples: u64,
    },
}

/// Why an analyser refused a configuration or a declared format (§5.1, §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AnalysisError {
    /// The declared rate is outside the linear-PCM boundary's `1..=384,000` Hz domain.
    ///
    /// The inner error is [`crate::PcmError`], reused rather than re-minted, so rate 0 and rate
    /// 384,001 refuse with exactly the type that boundary's own PCM-4 vector names.
    #[error(transparent)]
    UnsupportedRate(#[from] PcmError),
    /// A configured field is outside its domain.
    #[error(transparent)]
    Profile(#[from] ProfileError),
}

/// Why an analyser refused a frame (§7.3).
///
/// A refusal rejects the input and changes nothing: not the sequence expectation, not the stream
/// position, not an accumulator, and it enqueues no observation. A caller that fixes its input and
/// retries continues exactly where the stream stood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FrameError {
    /// The frame carries no samples, or more than [`MAX_FRAME_SAMPLES`] of them.
    ///
    /// Zero samples measure nothing, and accepting them silently would hide a broken seam.
    #[error("an analysis frame carries no samples, or more than {MAX_FRAME_SAMPLES} of them")]
    MalformedFrame,
    /// The frame's sequence violates §3.4: it is not strictly increasing, or it skips a number
    /// without the seam having flagged a discontinuity.
    #[error("an analysis frame's sequence is not the strictly increasing one §3.4 requires")]
    MalformedSequence,
    /// The frame's direction is not the one this analyser is bound to.
    #[error("an analysis frame carries the direction this analyser is not bound to")]
    DirectionMismatch,
}

/// One frame offered to an analyser (§3.3).
///
/// Samples are borrowed for the duration of the call and are never retained after
/// [`AudioAnalyzer::process`] returns: raw-audio non-retention is a design invariant, not an
/// optimisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnalysisFrame<'a> {
    direction: AudioDirection,
    sequence: u64,
    discontinuity: Option<DiscontinuityKind>,
    samples: &'a [i16],
}

impl<'a> AnalysisFrame<'a> {
    /// A frame that continues the stream: no break precedes it.
    #[must_use]
    pub const fn new(direction: AudioDirection, sequence: u64, samples: &'a [i16]) -> Self {
        Self {
            direction,
            sequence,
            discontinuity: None,
            samples,
        }
    }

    /// Declare the break that immediately precedes this frame.
    ///
    /// The reset it causes runs before this frame's own samples are consumed, so those samples
    /// open the new epoch (§7.1).
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

    /// The seam's frame number for this frame.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// The break immediately before this frame, if the seam declared one.
    #[must_use]
    pub const fn discontinuity(&self) -> Option<DiscontinuityKind> {
        self.discontinuity
    }

    /// The borrowed mono signed 16-bit samples, at the analyser's declared rate.
    #[must_use]
    pub const fn samples(&self) -> &'a [i16] {
        self.samples
    }
}

/// Why voice ended (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum VoiceEndCause {
    /// The configured hangover elapsed with no further active window.
    Hangover,
    /// A reset cut it: the epoch it belonged to no longer exists.
    Cut,
}

impl std::fmt::Display for VoiceEndCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hangover => f.write_str("hangover"),
            Self::Cut => f.write_str("cut"),
        }
    }
}

/// Why measurement restarted (§7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ResetCause {
    /// The caller asked for it.
    Requested,
    /// A format change re-derived every sample count against a new rate.
    FormatChange {
        /// The rate now in force.
        rate: u32,
    },
    /// The seam declared a break in the timeline.
    Discontinuity {
        /// What the seam said happened.
        kind: DiscontinuityKind,
    },
}

/// One typed fact an analyser produced (§5.3, §6, §7.1, §8.3).
///
/// Ordering within one completed window is fixed: [`Self::Window`] first, then a voice transition,
/// then [`Self::SilenceElapsed`]. Within one [`AudioAnalyzer::process`] call, windows complete in
/// stream order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Observation {
    /// A window of exactly `W` samples completed, with its accumulators and the five facts §5.3
    /// derives from them.
    Window {
        /// The window's index in the current epoch; its first sample is `index · W`.
        index: u64,
        /// The largest `|s|` in the window. `|−32768|` is 32,768, which is why this is `i32`.
        peak: i32,
        /// `Σ s`.
        sum: i64,
        /// `Σ s²`.
        energy: i64,
        /// How many samples sat at full scale, either sign.
        clipped: u32,
        /// `clipped >= clip_samples`: the window touched full scale often enough to distort.
        clipping: bool,
        /// More than half the window's energy sits in its single largest sample: a click.
        impulsive: bool,
        /// The DC-free variance meets the activation threshold, and the window is not impulsive.
        active: bool,
        /// The mean magnitude meets the DC threshold.
        dc_offset: bool,
        /// Nothing in the window rose above the silence floor.
        silent: bool,
    },
    /// Voice began at the first sample of an active window.
    VoiceStarted {
        /// The position in the current epoch, in samples.
        at_sample: u64,
    },
    /// Voice ended at the end of the last active window.
    VoiceEnded {
        /// The position in the current epoch, in samples.
        at_sample: u64,
        /// Whether the hangover elapsed or a reset cut it.
        cause: VoiceEndCause,
    },
    /// An unbroken silent run first reached the configured timeout.
    SilenceElapsed {
        /// The first sample of the run, in the current epoch.
        at_sample: u64,
    },
    /// Measurement restarted; sample positions after this belong to a new epoch and start at 0.
    Reset {
        /// What restarted it.
        cause: ResetCause,
    },
    /// Observations that had no queue slot (§8.3).
    ///
    /// Deterministic like everything else: the same input against the same capacity loses the same
    /// observations. The marker exists so an undersized queue is a visible, counted fact instead of
    /// a silent absence or an unbounded buffer.
    Lost {
        /// How many observations this marker stands for.
        count: u64,
    },
}

/// Every duration §5.1 configures, converted once into the sample counts §4 admits at runtime.
#[derive(Debug, Clone, Copy)]
struct Derived {
    window: u32,
    hangover: u64,
    silence_timeout: Option<u64>,
    queue_capacity: usize,
}

/// §4's exact conversion: `ceil(d · rate / 1000)`, in `u64`, with no floating point.
///
/// Written as the integer `div_ceil` the formula names rather than as `(x + 999) / 1000`, which is
/// the same value and one carry away from being a different one.
fn samples_for(duration_ms: u32, rate: u32) -> u64 {
    (u64::from(duration_ms) * u64::from(rate)).div_ceil(1_000)
}

impl Derived {
    /// Check every §5.1 domain and derive every count, or refuse naming the field.
    fn from(profile: &AnalysisProfile) -> Result<Self, AnalysisError> {
        // The rate first, through the shared boundary rather than a private domain check, so an
        // unsupported rate refuses with exactly that boundary's type (§3.2, CAP-C4).
        if profile.rate == 0 || profile.rate > MAX_SAMPLE_RATE {
            return Err(PcmError::UnsupportedSampleRate(profile.rate).into());
        }

        if profile.window_ms == 0 {
            return Err(field("window_ms", 0));
        }
        let window = samples_for(profile.window_ms, profile.rate);
        if window == 0 || window > u64::from(MAX_WINDOW_SAMPLES) {
            return Err(ProfileError::WindowSamples {
                window_ms: profile.window_ms,
                rate: profile.rate,
                samples: window,
            }
            .into());
        }
        // Proven inside `1..=MAX_WINDOW_SAMPLES` immediately above, so the narrowing cannot lose a
        // bit and the fallback is unreachable.
        let window = u32::try_from(window).unwrap_or(MAX_WINDOW_SAMPLES);

        check(
            i64::from(profile.activation_amplitude),
            "activation_amplitude",
            1,
            32_767,
        )?;
        check(
            i64::from(profile.silence_amplitude),
            "silence_amplitude",
            1,
            32_768,
        )?;
        check(
            i64::from(profile.impulse_amplitude),
            "impulse_amplitude",
            1,
            32_768,
        )?;
        check(i64::from(profile.dc_amplitude), "dc_amplitude", 1, 32_767)?;
        check(
            i64::from(profile.clip_samples),
            "clip_samples",
            1,
            i64::from(window),
        )?;
        check(
            i64::from(profile.queue_capacity),
            "queue_capacity",
            i64::from(MIN_QUEUE_CAPACITY),
            i64::from(MAX_QUEUE_CAPACITY),
        )?;

        let hangover = samples_for(profile.hangover_ms, profile.rate);
        if hangover > u64::from(u32::MAX) {
            return Err(ProfileError::DerivedCount {
                field: "hangover_ms",
                rate: profile.rate,
                samples: hangover,
            }
            .into());
        }

        let silence_timeout = match profile.silence_timeout_ms {
            None => None,
            Some(0) => return Err(field("silence_timeout_ms", 0)),
            Some(timeout_ms) => {
                let samples = samples_for(timeout_ms, profile.rate);
                if samples > u64::from(u32::MAX) {
                    return Err(ProfileError::DerivedCount {
                        field: "silence_timeout_ms",
                        rate: profile.rate,
                        samples,
                    }
                    .into());
                }
                Some(samples)
            }
        };

        Ok(Self {
            window,
            hangover,
            silence_timeout,
            // Proven inside `MIN_QUEUE_CAPACITY..=MAX_QUEUE_CAPACITY` above, so the fallback —
            // that ceiling, spelled as a `usize` — is unreachable.
            queue_capacity: usize::try_from(profile.queue_capacity).unwrap_or(4_096),
        })
    }
}

fn field(field: &'static str, value: i64) -> AnalysisError {
    ProfileError::Field { field, value }.into()
}

fn check(value: i64, name: &'static str, low: i64, high: i64) -> Result<(), AnalysisError> {
    if value < low || value > high {
        return Err(field(name, value));
    }
    Ok(())
}

/// The four §5.2 accumulators plus the position inside the window they cover.
#[derive(Debug, Clone, Copy, Default)]
struct WindowState {
    filled: u32,
    peak: i32,
    sum: i64,
    energy: i64,
    clipped: u32,
}

/// §6's edge-triggered activity state.
#[derive(Debug, Clone, Copy, Default)]
struct ActivityState {
    voiced: bool,
    inactive_run: u64,
    last_active_end: u64,
}

/// §6's silence timer, which is independent of activity.
#[derive(Debug, Clone, Copy, Default)]
struct SilenceState {
    run: u64,
    start: u64,
    fired: bool,
}

/// The deterministic frame processor of `docs/specs/call-audio-processing.md`.
///
/// Two analysers built from the same profile and fed the same input produce byte-identical drain
/// sequences on every architecture (CAP-D1). After construction the analyser performs no
/// allocation: not per frame, not per window, not per observation. Its size is a constant of its
/// configuration — independent of call duration, frame count and frame size (§8.1).
#[derive(Debug)]
pub struct AudioAnalyzer {
    profile: AnalysisProfile,
    derived: Derived,
    window: WindowState,
    activity: ActivityState,
    silence: SilenceState,
    /// The index of the window now filling, in the current epoch.
    window_index: u64,
    /// The last accepted frame's sequence, or `None` before the base is established.
    sequence: Option<u64>,
    queue: VecDeque<Observation>,
}

impl AudioAnalyzer {
    /// Validate a profile and build the analyser it configures.
    ///
    /// # Errors
    ///
    /// [`AnalysisError::UnsupportedRate`] for a rate outside `1..=384,000` Hz, and
    /// [`AnalysisError::Profile`] naming the first field outside the domain §5.1 states for it —
    /// both before anything is sized by the refused value.
    pub fn new(profile: AnalysisProfile) -> Result<Self, AnalysisError> {
        let derived = Derived::from(&profile)?;
        Ok(Self {
            profile,
            derived,
            window: WindowState::default(),
            activity: ActivityState::default(),
            silence: SilenceState::default(),
            window_index: 0,
            sequence: None,
            queue: VecDeque::with_capacity(derived.queue_capacity),
        })
    }

    /// The profile in force, with every field as it was configured.
    #[must_use]
    pub const fn profile(&self) -> AnalysisProfile {
        self.profile
    }

    /// The direction this analyser is bound to.
    #[must_use]
    pub const fn direction(&self) -> AudioDirection {
        self.profile.direction
    }

    /// The rate every count below was derived against.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.profile.rate
    }

    /// `W`: how many samples one window covers, derived by §4's formula.
    #[must_use]
    pub const fn window_samples(&self) -> u32 {
        self.derived.window
    }

    /// How many inactive samples end voice, derived by §4's formula.
    #[must_use]
    pub const fn hangover_samples(&self) -> u64 {
        self.derived.hangover
    }

    /// How many silent samples report elapsed silence, or `None` when the timer is disabled.
    #[must_use]
    pub const fn silence_timeout_samples(&self) -> Option<u64> {
        self.derived.silence_timeout
    }

    /// Whether voice is currently open — that is, a [`Observation::VoiceStarted`] has been
    /// enqueued with no matching [`Observation::VoiceEnded`] yet.
    #[must_use]
    pub const fn is_voiced(&self) -> bool {
        self.activity.voiced
    }

    /// Consume one frame.
    ///
    /// Accepted samples are measured immediately and every completed window's observations are
    /// enqueued before this returns. Nothing is retained: the borrow ends here.
    ///
    /// # Errors
    ///
    /// A [`FrameError`] for each refusal in §7.3. A refusal mutates nothing.
    pub fn process(&mut self, frame: &AnalysisFrame<'_>) -> Result<(), FrameError> {
        if frame.samples.is_empty() || frame.samples.len() > MAX_FRAME_SAMPLES {
            return Err(FrameError::MalformedFrame);
        }
        if frame.direction != self.profile.direction {
            return Err(FrameError::DirectionMismatch);
        }
        // After construction or any reset, the first accepted frame establishes the base with any
        // value; §3.4 then governs every frame after it.
        if let Some(previous) = self.sequence {
            // Sequence is strictly monotonic, flagged or not. A flagged frame need not skip a
            // number (CAP-S4); an unflagged gap is a broken upstream rather than a loss to smooth
            // over, because the seam always flags a gap.
            let acceptable = frame.sequence > previous
                && (frame.discontinuity.is_some() || frame.sequence == previous.saturating_add(1));
            if !acceptable {
                return Err(FrameError::MalformedSequence);
            }
        }

        if let Some(kind) = frame.discontinuity {
            self.restart(ResetCause::Discontinuity { kind });
        }
        self.sequence = Some(frame.sequence);
        for sample in frame.samples {
            self.consume(*sample);
        }
        Ok(())
    }

    /// Declare a new sample rate (§7.2).
    ///
    /// Every duration is re-derived against the new rate, every §5.1 domain is re-checked against
    /// the counts that produces, and a [`ResetCause::FormatChange`] reset opens the new epoch. A
    /// refused rate leaves the previous format in force, untouched: a malformed format change never
    /// half-applies.
    ///
    /// # Errors
    ///
    /// [`AnalysisError::UnsupportedRate`] for a rate outside the boundary's domain, and
    /// [`AnalysisError::Profile`] when re-derivation leaves a §5.1 domain — most often the derived
    /// window exceeding [`MAX_WINDOW_SAMPLES`].
    pub fn declare_format(&mut self, rate: u32) -> Result<(), AnalysisError> {
        let mut candidate = self.profile;
        candidate.rate = rate;
        // Derived before anything is assigned, so a refusal cannot leave half a format applied.
        let derived = Derived::from(&candidate)?;
        self.profile = candidate;
        self.derived = derived;
        self.restart(ResetCause::FormatChange { rate });
        Ok(())
    }

    /// Restart measurement at the caller's request (§7.1).
    ///
    /// Cuts open voice first, then announces the new epoch, then clears every runtime count. The
    /// configuration survives, and so does the observation queue: facts already earned are not
    /// destroyed by a reset.
    pub fn reset(&mut self) {
        self.restart(ResetCause::Requested);
    }

    /// Take every queued observation, in order, leaving the queue empty.
    ///
    /// Nothing else removes entries, and draining allocates nothing.
    pub fn drain(&mut self) -> Drain<'_, Observation> {
        self.queue.drain(..)
    }

    /// How many observations are waiting to be drained.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// The shared body of every reset cause (§7.1).
    fn restart(&mut self, cause: ResetCause) {
        if self.activity.voiced {
            self.enqueue(Observation::VoiceEnded {
                at_sample: self.activity.last_active_end,
                cause: VoiceEndCause::Cut,
            });
        }
        self.enqueue(Observation::Reset { cause });
        // The partial window is discarded unemitted: a fact computed over fewer than `W` samples
        // would compare against thresholds derived for `W` and be a different measurement (§5.2).
        self.window = WindowState::default();
        self.activity = ActivityState::default();
        self.silence = SilenceState::default();
        self.window_index = 0;
        self.sequence = None;
    }

    /// The fixed per-sample step of §8.2.
    fn consume(&mut self, sample: i16) {
        let value = i64::from(sample);
        let magnitude = i32::from(sample).abs();
        if magnitude > self.window.peak {
            self.window.peak = magnitude;
        }
        self.window.sum += value;
        self.window.energy += value * value;
        if sample == i16::MAX || sample == i16::MIN {
            self.window.clipped += 1;
        }
        self.window.filled += 1;
        if self.window.filled >= self.derived.window {
            self.complete_window();
        }
    }

    /// Derive §5.3's five facts, enqueue them, and run §6's two state machines.
    fn complete_window(&mut self) {
        let width = i64::from(self.derived.window);
        let span = u64::from(self.derived.window);
        let peak = i64::from(self.window.peak);
        let sum = self.window.sum;
        let energy = self.window.energy;

        // §5.2's width proof, asserted rather than assumed: `W <= 2^16` and `|s| <= 2^15` put every
        // quantity below inside `i64` with headroom, and a build that ever left that range must say
        // so rather than saturate silently (§4).
        debug_assert!(energy <= 1i64 << 46, "§5.2: energy <= W · 2^30 <= 2^46");
        debug_assert!(sum.abs() <= 1i64 << 31, "§5.2: |sum| <= W · 2^15 <= 2^31");

        let clipping = self.window.clipped >= self.profile.clip_samples;
        let impulsive =
            peak >= i64::from(self.profile.impulse_amplitude) && energy < 2 * peak * peak;
        let activation = i64::from(self.profile.activation_amplitude);
        // `(W·energy − sum²)/W²` is exactly the window's variance, so this is `variance >= A²` with
        // no division performed — and a constant signal has variance 0 and is not voice.
        let active =
            !impulsive && width * energy - sum * sum >= activation * activation * width * width;
        let dc_offset = sum.abs() >= i64::from(self.profile.dc_amplitude) * width;
        let silent = peak < i64::from(self.profile.silence_amplitude);

        let index = self.window_index;
        let start = index * span;
        let end = start + span;

        self.enqueue(Observation::Window {
            index,
            peak: self.window.peak,
            sum,
            energy,
            clipped: self.window.clipped,
            clipping,
            impulsive,
            active,
            dc_offset,
            silent,
        });

        if active {
            self.activity.inactive_run = 0;
            self.activity.last_active_end = end;
            if !self.activity.voiced {
                self.activity.voiced = true;
                self.enqueue(Observation::VoiceStarted { at_sample: start });
            }
        } else if self.activity.voiced {
            self.activity.inactive_run += span;
            if self.activity.inactive_run >= self.derived.hangover {
                self.activity.voiced = false;
                self.enqueue(Observation::VoiceEnded {
                    at_sample: self.activity.last_active_end,
                    cause: VoiceEndCause::Hangover,
                });
            }
        }

        if silent {
            if self.silence.run == 0 {
                self.silence.start = start;
            }
            self.silence.run += span;
            let reached = self
                .derived
                .silence_timeout
                .is_some_and(|timeout| self.silence.run >= timeout);
            if reached && !self.silence.fired {
                self.silence.fired = true;
                self.enqueue(Observation::SilenceElapsed {
                    at_sample: self.silence.start,
                });
            }
        } else {
            // The timer re-arms only after a non-silent window or a reset.
            self.silence.run = 0;
            self.silence.fired = false;
        }

        self.window = WindowState::default();
        self.window_index += 1;
    }

    /// §8.3's bounded ring: never blocks, never grows, and never loses an observation silently.
    fn enqueue(&mut self, observation: Observation) {
        if self.queue.len() >= self.derived.queue_capacity {
            match self.queue.back_mut() {
                // discard: §8.3. The newest retained entry is coalesced into loss accounting
                // rather than blocking the caller or growing past the configured capacity.
                Some(Observation::Lost { count }) => *count += 1,
                Some(newest) => *newest = Observation::Lost { count: 2 },
                // Unreachable: the capacity domain starts at 2, so a full queue has a back.
                None => {}
            }
            return;
        }
        self.queue.push_back(observation);
    }
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

    fn p8() -> AnalysisProfile {
        AnalysisProfile::new(AudioDirection::Inbound, 8_000)
    }

    /// §4's formula rounds up, so a window never covers less than the duration configured.
    #[test]
    fn durations_derive_by_rounding_up() {
        assert_eq!(samples_for(20, 8_000), 160);
        assert_eq!(samples_for(20, 8_193), 164);
        assert_eq!(samples_for(200, 8_000), 1_600);
        assert_eq!(samples_for(0, 8_000), 0);
    }

    /// §6: a hangover of zero ends voice at the first inactive window.
    #[test]
    fn a_zero_hangover_ends_voice_at_the_first_inactive_window() {
        let mut analyzer = AudioAnalyzer::new(p8().with_hangover_ms(0)).unwrap();
        let modulated: Vec<i16> = (0..160)
            .map(|index| if index % 2 == 0 { 8_192 } else { -8_192 })
            .collect();

        analyzer
            .process(&AnalysisFrame::new(AudioDirection::Inbound, 0, &modulated))
            .unwrap();
        analyzer
            .process(&AnalysisFrame::new(
                AudioDirection::Inbound,
                1,
                &[0i16; 160],
            ))
            .unwrap();

        let observations: Vec<Observation> = analyzer.drain().collect();
        assert_eq!(
            observations[3],
            Observation::VoiceEnded {
                at_sample: 160,
                cause: VoiceEndCause::Hangover,
            },
            "{observations:?}"
        );
    }

    /// §6: `silence_timeout_ms: None` disables the timer entirely.
    #[test]
    fn a_disabled_silence_timer_never_fires() {
        let mut analyzer = AudioAnalyzer::new(p8().with_silence_timeout_ms(None)).unwrap();
        for sequence in 0..200u64 {
            analyzer
                .process(&AnalysisFrame::new(
                    AudioDirection::Inbound,
                    sequence,
                    &[0i16; 160],
                ))
                .unwrap();
        }
        assert!(
            !analyzer
                .drain()
                .any(|o| matches!(o, Observation::SilenceElapsed { .. })),
            "a disabled timer has no count to reach"
        );
    }

    /// §6: the silence timer re-arms after a non-silent window, and fires again from the new run.
    #[test]
    fn the_silence_timer_re_arms_after_a_non_silent_window() {
        let mut analyzer = AudioAnalyzer::new(p8().with_silence_timeout_ms(Some(40))).unwrap();
        let quiet = AnalysisFrame::new(AudioDirection::Inbound, 0, &[0i16; 320]);
        analyzer.process(&quiet).unwrap();
        assert!(
            analyzer
                .drain()
                .any(|o| matches!(o, Observation::SilenceElapsed { at_sample: 0 })),
            "two silent windows reach the 320-sample timeout"
        );

        analyzer
            .process(&AnalysisFrame::new(
                AudioDirection::Inbound,
                1,
                &[1_000i16; 160],
            ))
            .unwrap();
        let _ = analyzer.drain();

        analyzer
            .process(&AnalysisFrame::new(
                AudioDirection::Inbound,
                2,
                &[0i16; 320],
            ))
            .unwrap();
        // Windows 0 and 1 were the first run, window 2 broke it, and the second run opens at
        // window 3 — sample 480, not where the first run started.
        assert!(
            analyzer
                .drain()
                .any(|o| matches!(o, Observation::SilenceElapsed { at_sample: 480 })),
            "the second run is measured from its own first sample"
        );
    }

    /// §8.1: the queue is preallocated, so nothing about running a call grows it.
    #[test]
    fn the_observation_queue_never_grows_past_its_capacity() {
        let mut analyzer = AudioAnalyzer::new(p8().with_queue_capacity(2)).unwrap();
        let allocated = analyzer.queue.capacity();
        for sequence in 0..50u64 {
            analyzer
                .process(&AnalysisFrame::new(
                    AudioDirection::Inbound,
                    sequence,
                    &[0i16; 160],
                ))
                .unwrap();
        }
        assert_eq!(analyzer.queued(), 2);
        assert_eq!(
            analyzer.queue.capacity(),
            allocated,
            "no allocation after construction"
        );
    }
}
