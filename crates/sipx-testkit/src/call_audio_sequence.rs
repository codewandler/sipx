//! Adversarial call-audio programs, their invariant oracle, and the seed corpus (`M-61`).
//!
//! `sipx-audio`'s `CAP-*` and `CAL-*` fixtures cover the inputs
//! [`docs/specs/call-audio-processing.md`](../../../../docs/specs/call-audio-processing.md)
//! thought of. This is the instrument for the ones it did not: a byte string decodes into a program
//! over the analyser's whole input vocabulary — frames of hostile audio in every supported PCM
//! format, declared format changes, requested resets, sequence gaps and replays, and a consumer
//! that stops draining — so a fuzzer's budget is spent inside §5's arithmetic and §12's calibration
//! rather than on bytes that decode into nothing.
//!
//! # The oracle is not "did it panic"
//!
//! The analyser is total. Almost any sequence "succeeds", and the failures that matter are quiet: a
//! queue that grew, a refusal that moved the stream position, a threshold that left its declared
//! interval, a window whose reported facts disagree with its own accumulators, a position that
//! names audio nobody fed, voice left latched across a reset, a signal report claiming coverage it
//! does not have. [`Invariant`] enumerates them and [`run`] returns them as data; the fuzz target
//! is the thin part that turns a finding into a crash libFuzzer can minimise, and the replay tests
//! next to the code assert the same values without one.
//!
//! # Two lanes, because one of them is the answer
//!
//! Every program is driven through **two** analysers fed byte-identical input.
//!
//! The *subject* carries §12.12's reference calibration profile and a deliberately shallow
//! observation queue, so calibration and §8.3's backpressure are both under test.
//!
//! The *reference* carries no calibration at all and is fixed at the subject's `floor_amplitude`,
//! with a queue deep enough never to coalesce, and it is drained after every step. It is where
//! every claim about *positions* is checked: a lane whose queue coalesces cannot say which epoch a
//! surviving observation belongs to, because the `Reset` that opened it may be one of the ones the
//! queue folded into a counted marker. Checking a position against a guessed epoch is how an
//! oracle reports the defect it has rather than the one the code has.
//!
//! §12.11's first clause — *"the set of voice intervals adaptation can open is a subset of one that
//! does not adapt at all"* — needs no lane at all: a window the subject calls `active` must satisfy
//! the activation predicate at `floor_amplitude`, and that is decidable from the window's own
//! published accumulators. CAL-9, extended from one fixture to every program a fuzzer can write.
//!
//! Nothing here reads a clock, spawns a task or touches a socket: the analyser's contract is
//! sans-I/O, and so is its adversary.

use std::path::PathBuf;

use sipx_audio::analysis::{
    AnalysisFrame, AnalysisProfile, AudioAnalyzer, AudioDirection, CalibrationProfile,
    DiscontinuityKind, MAX_FRAME_SAMPLES, Observation, window_deviation,
};
use sipx_audio::signal::{SignalObservation, SignalReportProfile, SignalReporter};
use sipx_audio::{Pcm, PcmEncoding, PcmFormat};

// ---- vocabulary ----

/// The direction every program runs in.
///
/// One direction, deliberately: §3.1 binds an analyser to one side at construction and refuses the
/// other's frames, so a program that switched direction would be a program about §7.3's refusal and
/// nothing else. That refusal has its own fixture.
pub const DIRECTION: AudioDirection = AudioDirection::Inbound;

/// The rate the subject is constructed at, and the one every derived count starts from.
pub const BASE_RATE: u32 = 8_000;

/// Every rate a program may declare, refused ones included.
///
/// The two ends of the [`sipx_audio::pcm`] domain (0 and 384,001) are here so a program can ask for
/// a format change that must not half-apply (§7.2, CAP-F2); 8,193 is CAP-F3's rate, whose derived
/// window rounds up rather than truncating; 384,000 with a 20 ms window derives 7,680 samples and
/// is accepted, while the same window at a rate that would derive past 65,536 is refused by
/// arithmetic rather than by a table.
pub const RATES: [u32; 12] = [
    0, 1, 8_000, 8_193, 11_025, 16_000, 32_768, 44_100, 48_000, 96_000, 384_000, 384_001,
];

/// Every PCM representation the application boundary supports.
///
/// Exhaustive over [`PcmEncoding`] on purpose: "every supported PCM format" is `(rate, encoding)`,
/// and the eight-bit half reaches the analyser only after the boundary's own conversion — the same
/// route `M-43` gives the seam. A program that only ever minted `i16` would leave that conversion
/// unfuzzed.
pub const ENCODINGS: [PcmEncoding; 2] = [PcmEncoding::Unsigned8, PcmEncoding::Signed16];

/// The amplitudes a pattern is drawn at.
///
/// Chosen at the edges the predicates of §5.3 turn on: the silence floor (64) and one below it, the
/// reference activation amplitude (2,048) and one below it, the impulse amplitude (16,384), and
/// full scale, where `clipped` starts counting.
pub const AMPLITUDES: [i32; 12] = [
    0, 1, 63, 64, 512, 1_000, 2_047, 2_048, 8_192, 16_384, 32_766, 32_767,
];

/// The frame lengths a program may offer, in samples.
///
/// Both refusals of §7.3 are reachable — 0 samples and one past [`MAX_FRAME_SAMPLES`] — because a
/// refusal that mutated state is exactly the defect this harness is looking for, and it can only be
/// seen by asking for one.
pub const FRAME_SAMPLES: [usize; 12] = [
    0,
    1,
    2,
    7,
    40,
    159,
    160,
    161,
    320,
    977,
    MAX_FRAME_SAMPLES,
    MAX_FRAME_SAMPLES + 1,
];

/// The break vocabulary a program may declare, which is the seam's own (§3.3).
pub const KINDS: [DiscontinuityKind; 3] = [
    DiscontinuityKind::Loss,
    DiscontinuityKind::Overflow,
    DiscontinuityKind::Realign,
];

/// How the samples of one frame are shaped.
///
/// Each is a fact of §5.3 written as audio: a shape that reaches a predicate, or one that a
/// predicate must keep out.
///
/// Exhaustive by design: a new shape is a new adversary and belongs in this list, but nothing
/// outside this crate matches on it — the harness folds a fuzzer's byte into it and never back
/// out, so adding one changes what is fuzzed and no caller's code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// Digital silence.
    Zero,
    /// A stuck DAC: the same positive value forever. Variance exactly 0, so never voice (CAP-W3).
    Constant,
    /// The same, negative.
    NegativeConstant,
    /// Alternating `±v`, whose window deviation is exactly `v` (§12.12).
    Alternating,
    /// One full-scale sample in an otherwise silent window: a click, not a signal (CAP-W4).
    Impulse,
    /// Alternating [`i16::MIN`]/[`i16::MAX`]: the corner §5.2's width proof is stated at.
    ExtremeAlternating,
    /// A rising ramp that wraps, which is neither constant nor symmetric.
    Ramp,
    /// `+v` for the first half and `−v` for the second: a DC step inside one window.
    Step,
    /// Alternating `±v` with every eighth sample driven to full scale.
    ImpulseTrain,
    /// A deterministic pseudo-random fill in `−v..=v`, so no shape above is the only shape.
    Noise,
}

/// How many patterns [`Pattern::from_byte`] folds onto.
const PATTERNS: u8 = 10;

impl Pattern {
    /// Fold one fuzzer byte onto a pattern. Total: every byte is a pattern.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Self {
        match byte % PATTERNS {
            0 => Self::Zero,
            1 => Self::Constant,
            2 => Self::NegativeConstant,
            3 => Self::Alternating,
            4 => Self::Impulse,
            5 => Self::ExtremeAlternating,
            6 => Self::Ramp,
            7 => Self::Step,
            8 => Self::ImpulseTrain,
            _ => Self::Noise,
        }
    }

    /// The byte this pattern decodes from.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::Constant => 1,
            Self::NegativeConstant => 2,
            Self::Alternating => 3,
            Self::Impulse => 4,
            Self::ExtremeAlternating => 5,
            Self::Ramp => 6,
            Self::Step => 7,
            Self::ImpulseTrain => 8,
            Self::Noise => 9,
        }
    }

    /// Mint `count` samples of this shape at `amplitude`.
    #[must_use]
    pub fn samples(self, count: usize, amplitude: i32) -> Vec<i16> {
        let level = i16::try_from(amplitude.clamp(0, i32::from(i16::MAX))).unwrap_or(i16::MAX);
        let mut samples = Vec::with_capacity(count);
        // A deterministic fill: the same program mints the same audio on every machine, which is
        // what makes a minimised crash replayable (§4).
        let mut state: u32 = 0x2545_F491;
        for index in 0..count {
            let value = match self {
                Self::Zero => 0,
                Self::Constant => level,
                Self::NegativeConstant => level.saturating_neg(),
                Self::Alternating => {
                    if index % 2 == 0 {
                        level
                    } else {
                        level.saturating_neg()
                    }
                }
                Self::Impulse => {
                    if index == count / 2 {
                        i16::MAX
                    } else {
                        0
                    }
                }
                Self::ExtremeAlternating => {
                    if index % 2 == 0 {
                        i16::MAX
                    } else {
                        i16::MIN
                    }
                }
                Self::Ramp => i16::try_from(
                    i64::try_from(index).unwrap_or(0) % (i64::from(level).max(1) * 2 + 1)
                        - i64::from(level),
                )
                .unwrap_or(0),
                Self::Step => {
                    if index * 2 < count {
                        level
                    } else {
                        level.saturating_neg()
                    }
                }
                Self::ImpulseTrain => {
                    if index % 8 == 0 {
                        i16::MIN
                    } else if index % 2 == 0 {
                        level
                    } else {
                        level.saturating_neg()
                    }
                }
                Self::Noise => {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let span = i64::from(level) * 2 + 1;
                    i16::try_from(i64::from(state >> 16) % span - i64::from(level)).unwrap_or(0)
                }
            };
            samples.push(value);
        }
        samples
    }
}

// ---- the program ----

/// One step of an adversarial program.
///
/// Exhaustive by design: the vocabulary is the analyser's whole input surface (§1) plus the two
/// ways a caller can break §3.4's sequence rule. Anything outside it would be a different contract,
/// not a new adversary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Offer one frame, continuing the stream.
    Frame {
        /// Which shape its samples take.
        pattern: Pattern,
        /// Index into [`AMPLITUDES`].
        amplitude: u8,
        /// Index into [`FRAME_SAMPLES`].
        samples: u8,
    },
    /// Offer one frame preceded by a declared break (§7.1).
    Broken {
        /// Index into [`KINDS`].
        kind: u8,
        /// Which shape its samples take.
        pattern: Pattern,
        /// Index into [`AMPLITUDES`].
        amplitude: u8,
    },
    /// Skip `skip + 1` sequence numbers without flagging a break, which §3.4 refuses.
    Gap {
        /// How far to jump, less one.
        skip: u8,
        /// Which shape the refused frame's samples take.
        pattern: Pattern,
    },
    /// Offer a sequence number already used, which §3.4 refuses whether flagged or not.
    Replay {
        /// How far back, less one.
        back: u8,
        /// Whether to flag the replay, which changes nothing.
        flagged: bool,
    },
    /// Declare a new sample rate (§7.2). Index into [`RATES`].
    DeclareFormat {
        /// Index into [`RATES`].
        rate: u8,
    },
    /// Mint later frames in a different source PCM format, converted through the application
    /// boundary exactly as the seam converts (`M-43`).
    SourceFormat {
        /// Index into [`RATES`].
        rate: u8,
        /// Index into [`ENCODINGS`].
        encoding: u8,
    },
    /// Restart measurement at the caller's request (§7.1).
    Reset,
    /// Take everything the subject has queued. Between two of these the consumer is not reading,
    /// which is §8.3's backpressure.
    Drain,
}

/// How many opcodes [`Event`] folds onto.
const OPCODES: u8 = 8;

/// Bytes per encoded event.
const RECORD: usize = 4;

/// A decoded fuzzer input: the steps, in the order they are driven.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Program {
    /// The events, in order.
    pub events: Vec<Event>,
}

impl Program {
    /// Decode a fuzzer input.
    ///
    /// Total: every byte string is a program. A trailing partial record is ignored, which is what
    /// lets libFuzzer shrink an input a byte at a time without the tail turning into noise.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Self {
        let events = bytes
            .chunks_exact(RECORD)
            .filter_map(|chunk| match chunk {
                &[op, a, b, c] => Some(decode_event(op, a, b, c)),
                _ => None,
            })
            .collect();
        Self { events }
    }

    /// Encode a program back to the bytes that decode to it.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.events.len() * RECORD);
        for event in &self.events {
            bytes.extend_from_slice(&encode_event(*event));
        }
        bytes
    }
}

fn decode_event(op: u8, a: u8, b: u8, c: u8) -> Event {
    match op % OPCODES {
        0 => Event::Frame {
            pattern: Pattern::from_byte(a),
            amplitude: b,
            samples: c,
        },
        1 => Event::Broken {
            kind: a,
            pattern: Pattern::from_byte(b),
            amplitude: c,
        },
        2 => Event::Gap {
            skip: a,
            pattern: Pattern::from_byte(b),
        },
        3 => Event::Replay {
            back: a,
            flagged: b & 1 != 0,
        },
        4 => Event::DeclareFormat { rate: a },
        5 => Event::SourceFormat {
            rate: a,
            encoding: b,
        },
        6 => Event::Reset,
        _ => Event::Drain,
    }
}

fn encode_event(event: Event) -> [u8; RECORD] {
    match event {
        Event::Frame {
            pattern,
            amplitude,
            samples,
        } => [0, pattern.to_byte(), amplitude, samples],
        Event::Broken {
            kind,
            pattern,
            amplitude,
        } => [1, kind, pattern.to_byte(), amplitude],
        Event::Gap { skip, pattern } => [2, skip, pattern.to_byte(), 0],
        Event::Replay { back, flagged } => [3, back, u8::from(flagged), 0],
        Event::DeclareFormat { rate } => [4, rate, 0, 0],
        Event::SourceFormat { rate, encoding } => [5, rate, encoding, 0],
        Event::Reset => [6, 0, 0, 0],
        Event::Drain => [7, 0, 0, 0],
    }
}

// ---- the oracle ----

/// A promise the processing contract makes that a program broke.
///
/// Exhaustive by design: each names one clause of the processing contract, and a finding outside
/// this list is a clause the contract does not make — which is a specification change rather than
/// a new variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invariant {
    /// §8.3: the observation queue held more than its configured capacity.
    QueueOverCapacity,
    /// §7.3, §7.2: a refusal changed the sequence expectation, the declared rate, the thresholds or
    /// the queue. *"The caller that retries after fixing its input continues exactly where the
    /// stream stood."*
    RefusalMutatedState,
    /// §12.3, §12.5: the effective activation amplitude left `[floor, ceiling]`, or moved at all
    /// with no calibration profile attached.
    ThresholdOutOfBounds,
    /// §4: a derived count is not `ceil(d · rate / 1000)` against the rate in force.
    DerivedCountDisagrees,
    /// §5.2: a reported accumulator is outside the width proof's range for the window it covers.
    AccumulatorOutOfDomain,
    /// §5.3: a reported fact is not what its own accumulators say it is.
    PredicateDisagrees,
    /// §12.11: a window adaptation called `active` that the analyser fixed at `floor_amplitude`
    /// did not — calibration was more sensitive than its own floor.
    MoreSensitiveThanFloor,
    /// §5.2, §6: a position or window index names audio nobody fed, or does not advance.
    PositionRegressed,
    /// §7.1: a reset was announced without cutting the voice it interrupted.
    VoiceLatchedAcrossReset,
    /// `M-59`: a signal report claims coverage its epoch does not have.
    ReportOverclaimsCoverage,
}

impl std::fmt::Display for Invariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::QueueOverCapacity => "queue over capacity",
            Self::RefusalMutatedState => "refusal mutated state",
            Self::ThresholdOutOfBounds => "threshold out of bounds",
            Self::DerivedCountDisagrees => "derived count disagrees",
            Self::AccumulatorOutOfDomain => "accumulator out of domain",
            Self::PredicateDisagrees => "predicate disagrees",
            Self::MoreSensitiveThanFloor => "more sensitive than floor",
            Self::PositionRegressed => "position regressed",
            Self::VoiceLatchedAcrossReset => "voice latched across reset",
            Self::ReportOverclaimsCoverage => "report overclaims coverage",
        };
        f.write_str(name)
    }
}

/// One broken promise, with enough detail to name a defect from.
#[derive(Debug, Clone)]
pub struct Violation {
    /// Which step of the program, or [`usize::MAX`] for the final drain that follows it.
    pub step: usize,
    /// Which promise.
    pub invariant: Invariant,
    /// What was seen.
    pub detail: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "step {}: {}: {}", self.step, self.invariant, self.detail)
    }
}

/// What driving a program produced.
#[derive(Debug, Clone)]
pub struct Run {
    /// One line per step: the event, what the analyser did with it, and what it is holding.
    pub trace: Vec<String>,
    /// Every promise broken, in the order they were noticed.
    pub violations: Vec<Violation>,
    /// How many observations the subject drained over the whole program.
    pub observations: u64,
    /// How many frames the subject accepted.
    pub accepted: u64,
    /// How many frames it refused.
    pub refused: u64,
}

// ---- the driver ----

/// The subject's observation queue.
///
/// Deliberately shallow: §8.3's coalescing is the behaviour under test, and a queue a program
/// cannot fill would leave the `Lost` accounting unreached. Two is the domain's own floor.
const SUBJECT_CAPACITY: u32 = 4;

/// The reference lane's queue, deep enough that a program cannot make it lose a window.
const REFERENCE_CAPACITY: u32 = 4_096;

/// §12.12's reference calibration profile, which the subject carries.
fn calibration() -> CalibrationProfile {
    CalibrationProfile::new()
}

/// The subject profile: §11.1's `P8`, calibrated, with a queue a consumer can overrun.
#[must_use]
pub fn subject_profile() -> AnalysisProfile {
    AnalysisProfile::new(DIRECTION, BASE_RATE)
        .with_queue_capacity(SUBJECT_CAPACITY)
        .with_calibration(Some(calibration()))
}

/// The reference profile: the same measurement fixed at the subject's floor, never coalescing.
///
/// §12.11's first clause is a claim about *these two*, so the reference is not a second guess at
/// the audio — it is the analyser the claim names.
#[must_use]
pub fn reference_profile() -> AnalysisProfile {
    AnalysisProfile::new(DIRECTION, BASE_RATE)
        .with_activation_amplitude(calibration().floor_amplitude())
        .with_queue_capacity(REFERENCE_CAPACITY)
}

/// What one lane holds.
struct Lane {
    analyzer: AudioAnalyzer,
    reporter: SignalReporter,
    /// Samples the analyser accepted in each epoch, indexed by epoch.
    epoch_samples: Vec<u64>,
    /// The epoch observations drained from this lane currently belong to.
    epoch: usize,
    /// The last window index seen in the current epoch.
    last_window: Option<u64>,
    /// Whether a `VoiceStarted` is outstanding on the drained stream.
    voiced: bool,
}

impl Lane {
    fn new(profile: AnalysisProfile) -> Option<Self> {
        Some(Self {
            analyzer: AudioAnalyzer::new(profile).ok()?,
            reporter: SignalReporter::new(SignalReportProfile::new(profile)).ok()?,
            epoch_samples: vec![0],
            epoch: 0,
            last_window: None,
            voiced: false,
        })
    }

    /// Account for `samples` reaching the analyser in the epoch now filling.
    fn fed(&mut self, samples: u64) {
        if let Some(current) = self.epoch_samples.last_mut() {
            *current = current.saturating_add(samples);
        }
    }

    /// Open a new epoch's accounting. Called for every reset the analyser will announce.
    fn epoch_opened(&mut self) {
        self.epoch_samples.push(0);
    }
}

/// Drive an adversarial program and report what it broke.
///
/// Never panics and never asserts: findings come back in [`Run::violations`] so the replay tests
/// can assert on them and the fuzz target can turn them into a crash.
#[must_use]
pub fn run(program: &Program) -> Run {
    // Unreachable: both profiles above are inside every domain §5.1 states. Reported rather than
    // unwrapped, because a harness that panics is a harness whose findings are its own.
    let Some(mut driver) = Driver::new() else {
        return Run {
            trace: vec!["the reference profiles were refused".to_owned()],
            violations: Vec::new(),
            observations: 0,
            accepted: 0,
            refused: 0,
        };
    };
    for (step, event) in program.events.iter().enumerate() {
        driver.step(step, *event);
    }
    driver.drain(usize::MAX);
    driver.finish()
}

struct Driver {
    subject: Lane,
    reference: Lane,
    /// The next sequence number a continuing frame carries.
    sequence: u64,
    /// The source PCM format later frames are minted in, before the boundary's conversion.
    source: PcmFormat,
    trace: Vec<String>,
    violations: Vec<Violation>,
    observations: u64,
    accepted: u64,
    refused: u64,
}

impl Driver {
    fn new() -> Option<Self> {
        Some(Self {
            subject: Lane::new(subject_profile())?,
            reference: Lane::new(reference_profile())?,
            sequence: 0,
            source: PcmFormat::new(BASE_RATE, PcmEncoding::Signed16).ok()?,
            trace: Vec::new(),
            violations: Vec::new(),
            observations: 0,
            accepted: 0,
            refused: 0,
        })
    }

    fn note(&mut self, step: usize, invariant: Invariant, detail: String) {
        self.violations.push(Violation {
            step,
            invariant,
            detail,
        });
    }

    fn step(&mut self, step: usize, event: Event) {
        match event {
            Event::Frame {
                pattern,
                amplitude,
                samples,
            } => {
                let samples = self.mint(pattern, amplitude, samples);
                let sequence = self.sequence;
                self.offer(step, sequence, None, &samples);
            }
            Event::Broken {
                kind,
                pattern,
                amplitude,
            } => {
                let kind = pick(&KINDS, kind).unwrap_or(DiscontinuityKind::Loss);
                // The frame's own samples open the epoch the break announces (§7.1), so a break is
                // one full frame length by construction.
                let samples = self.mint(pattern, amplitude, 6);
                let sequence = self.sequence;
                self.offer(step, sequence, Some(kind), &samples);
            }
            Event::Gap { skip, pattern } => {
                let samples = self.mint(pattern, 8, 6);
                let sequence = self.sequence.saturating_add(u64::from(skip)).max(1);
                // §3.4: the seam always flags a gap, so an unflagged one is refused. Whatever it
                // carried, nothing about the stream may move.
                self.offer(step, sequence, None, &samples);
            }
            Event::Replay { back, flagged } => {
                let samples = self.mint(Pattern::Alternating, 7, 6);
                let sequence = self
                    .sequence
                    .saturating_sub(u64::from(back).saturating_add(1));
                let kind = flagged.then_some(DiscontinuityKind::Overflow);
                self.offer(step, sequence, kind, &samples);
            }
            Event::DeclareFormat { rate } => self.declare(step, rate),
            Event::SourceFormat { rate, encoding } => {
                let rate = pick(&RATES, rate).unwrap_or(BASE_RATE);
                let encoding = pick(&ENCODINGS, encoding).unwrap_or(PcmEncoding::Signed16);
                match PcmFormat::new(rate, encoding) {
                    Ok(format) => {
                        self.source = format;
                        self.trace
                            .push(format!("step {step}: source format {rate} Hz {encoding:?}"));
                    }
                    Err(refusal) => self
                        .trace
                        .push(format!("step {step}: source format refused: {refusal}")),
                }
            }
            Event::Reset => {
                self.subject.analyzer.reset();
                self.reference.analyzer.reset();
                self.subject.epoch_opened();
                self.reference.epoch_opened();
                self.sequence = 0;
                self.trace.push(format!("step {step}: reset"));
                self.check_queue(step);
                self.harvest_reference(step);
            }
            Event::Drain => self.drain(step),
        }
    }

    /// Mint one frame's samples, through the application boundary's own conversion when the source
    /// format is not the analyser's (§3.2, `M-43`).
    fn mint(&self, pattern: Pattern, amplitude: u8, samples: u8) -> Vec<i16> {
        let amplitude = pick(&AMPLITUDES, amplitude).unwrap_or(0);
        let count = pick(&FRAME_SAMPLES, samples).unwrap_or(160);
        let target = self.subject.analyzer.sample_rate();
        if self.source.sample_rate() == target && self.source.encoding() == PcmEncoding::Signed16 {
            return pattern.samples(count, amplitude);
        }
        // The conversion is a rate change, so a source count is not a frame length: 160 samples at
        // 1 Hz become 1,280,000 at 8,000, and the largest count in `FRAME_SAMPLES` becomes half a
        // billion. That is an adversary against the *harness* and not against the analyser — the
        // frame is refused for length either way (§7.3) — so the source count is capped at whatever
        // converts to one sample past the §3.3 ceiling. The ceiling stays reachable; the half a
        // billion does not.
        let cap = u64::try_from(MAX_FRAME_SAMPLES.saturating_add(1))
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::from(self.source.sample_rate()))
            .checked_div(u64::from(target))
            .unwrap_or(u64::MAX)
            .max(1);
        let count = count.min(usize::try_from(cap).unwrap_or(usize::MAX));
        let pcm = Pcm::from_i16(self.source, pattern.samples(count, amplitude));
        // A conversion the boundary refuses leaves the frame empty, which the analyser refuses in
        // its own words rather than this harness inventing an outcome.
        pcm.to_i16(target).unwrap_or_default()
    }

    /// Offer one frame to both lanes and check what §7.3 promises about the answer.
    fn offer(
        &mut self,
        step: usize,
        sequence: u64,
        kind: Option<DiscontinuityKind>,
        samples: &[i16],
    ) {
        let before = Snapshot::of(&self.subject.analyzer);
        let mut frame = AnalysisFrame::new(DIRECTION, sequence, samples);
        if let Some(kind) = kind {
            frame = frame.with_discontinuity(kind);
        }
        let subject = self.subject.analyzer.process(&frame);
        let reference = self.reference.analyzer.process(&frame);

        if subject.is_ok() != reference.is_ok() {
            self.note(
                step,
                Invariant::PredicateDisagrees,
                format!(
                    "one lane accepted a frame the other refused: subject {subject:?}, reference \
                     {reference:?}"
                ),
            );
        }

        match subject {
            Ok(()) => {
                self.accepted = self.accepted.saturating_add(1);
                if kind.is_some() {
                    self.subject.epoch_opened();
                    self.reference.epoch_opened();
                }
                let count = u64::try_from(samples.len()).unwrap_or(0);
                self.subject.fed(count);
                self.reference.fed(count);
                // An accepted frame establishes the expectation whether it continued the stream or
                // opened a new epoch (§3.4), so the next continuing frame is always its successor —
                // including after a refusal, which moved nothing.
                self.sequence = sequence.saturating_add(1);
                self.trace.push(format!(
                    "step {step}: frame seq {sequence} {} samples{} -> accepted, queued {}",
                    samples.len(),
                    kind.map_or(String::new(), |kind| format!(" ({kind})")),
                    self.subject.analyzer.queued()
                ));
            }
            Err(refusal) => {
                self.refused = self.refused.saturating_add(1);
                let after = Snapshot::of(&self.subject.analyzer);
                if before != after {
                    self.note(
                        step,
                        Invariant::RefusalMutatedState,
                        format!("{refusal} moved the stream: {before:?} -> {after:?}"),
                    );
                }
                self.trace.push(format!(
                    "step {step}: frame seq {sequence} {} samples -> {refusal}",
                    samples.len()
                ));
            }
        }
        self.check_queue(step);
        self.check_thresholds(step);
        self.harvest_reference(step);
    }

    /// Declare a format, and check §7.2's promise that a refused one never half-applies.
    ///
    /// Both lanes are read out *first*. An observation carries its window index but not the width
    /// that index counts in, so a batch spanning a format change cannot be attributed to a width at
    /// all — and an oracle that guessed would report the width it guessed rather than the one the
    /// analyser used. Only the caller knows when the width changed, so the caller reads first. What
    /// this costs is one scenario the harness cannot pose: a queue held across a format change. The
    /// capacity bound is checked on every event regardless, which is the part of that scenario a
    /// program could break.
    fn declare(&mut self, step: usize, rate: u8) {
        self.drain(step);
        self.harvest_reference(step);
        let rate = pick(&RATES, rate).unwrap_or(BASE_RATE);
        let before = Snapshot::of(&self.subject.analyzer);
        let subject = self.subject.analyzer.declare_format(rate);
        let reference = self.reference.analyzer.declare_format(rate);
        if subject.is_ok() != reference.is_ok() {
            self.note(
                step,
                Invariant::PredicateDisagrees,
                format!("one lane accepted rate {rate} and the other did not"),
            );
        }
        match subject {
            Ok(()) => {
                self.subject.epoch_opened();
                self.reference.epoch_opened();
                self.sequence = 0;
                self.trace.push(format!("step {step}: format {rate} Hz"));
            }
            Err(refusal) => {
                let after = Snapshot::of(&self.subject.analyzer);
                if before != after {
                    self.note(
                        step,
                        Invariant::RefusalMutatedState,
                        format!(
                            "refused rate {rate} ({refusal}) still moved: {before:?} -> {after:?}"
                        ),
                    );
                }
                self.trace
                    .push(format!("step {step}: format {rate} Hz refused: {refusal}"));
            }
        }
        self.check_queue(step);
        self.check_derived(step);
    }

    /// §8.3: nothing a program can do makes the queue hold more than its capacity.
    fn check_queue(&mut self, step: usize) {
        let capacity = usize::try_from(SUBJECT_CAPACITY).unwrap_or(usize::MAX);
        let queued = self.subject.analyzer.queued();
        if queued > capacity {
            self.note(
                step,
                Invariant::QueueOverCapacity,
                format!("{queued} observations queued in {capacity} slots"),
            );
        }
    }

    /// §12.3, §12.5: the effective threshold never leaves its declared interval, and a lane with no
    /// calibration profile never moves at all.
    fn check_thresholds(&mut self, step: usize) {
        let thresholds = self.subject.analyzer.thresholds();
        let activation = thresholds.activation_amplitude();
        if let Some(calibration) = thresholds.calibration()
            && (activation < calibration.floor_amplitude()
                || activation > calibration.ceiling_amplitude())
        {
            self.note(
                step,
                Invariant::ThresholdOutOfBounds,
                format!(
                    "{activation} outside {}..={}",
                    calibration.floor_amplitude(),
                    calibration.ceiling_amplitude()
                ),
            );
        }
        let fixed = self.reference.analyzer.thresholds();
        if fixed.activation_amplitude() != fixed.profile().activation_amplitude() {
            self.note(
                step,
                Invariant::ThresholdOutOfBounds,
                format!(
                    "an analyser with no calibration profile moved to {}",
                    fixed.activation_amplitude()
                ),
            );
        }
        self.check_derived(step);
    }

    /// §4: every count in force is `ceil(d · rate / 1000)` against the rate in force.
    fn check_derived(&mut self, step: usize) {
        let thresholds = self.subject.analyzer.thresholds();
        let profile = thresholds.profile();
        let rate = profile.rate();
        let expected_window = samples_for(profile.window_ms(), rate);
        if u64::from(thresholds.window_samples()) != expected_window {
            self.note(
                step,
                Invariant::DerivedCountDisagrees,
                format!(
                    "window {} at {rate} Hz, expected {expected_window}",
                    thresholds.window_samples()
                ),
            );
        }
        if thresholds.hangover_samples() != samples_for(profile.hangover_ms(), rate) {
            self.note(
                step,
                Invariant::DerivedCountDisagrees,
                format!("hangover {} at {rate} Hz", thresholds.hangover_samples()),
            );
        }
        if let (Some(calibration), Some(warmup), Some(update)) = (
            thresholds.calibration(),
            thresholds.calibration_samples(),
            thresholds.update_samples(),
        ) && (warmup != samples_for(calibration.calibration_ms(), rate)
            || update != samples_for(calibration.update_ms(), rate))
        {
            self.note(
                step,
                Invariant::DerivedCountDisagrees,
                format!("calibration {warmup}/{update} at {rate} Hz"),
            );
        }
    }

    /// Read the reference lane and record what it called `active`, per epoch and window.
    fn harvest_reference(&mut self, step: usize) {
        let window = self.reference.analyzer.window_samples();
        let drained: Vec<Observation> = self.reference.analyzer.drain().collect();
        for observation in drained {
            self.check_position(step, &observation, window);
            let reporter = &mut self.reference.reporter;
            if let Some(report) = reporter.observe(&observation, window) {
                self.check_report(step, &report);
            }
        }
    }

    /// Where an observation sits, checked on the lane that never loses one.
    ///
    /// Every position is relative to the epoch that opened it (§6), so this is only answerable on a
    /// stream with no hole in it. The reference lane is that stream: its queue is deep enough that
    /// a `Lost` marker is itself a finding.
    fn check_position(&mut self, step: usize, observation: &Observation, window: u32) {
        let span = u64::from(window);
        let epoch = self.reference.epoch;
        let fed = self
            .reference
            .epoch_samples
            .get(epoch)
            .copied()
            .unwrap_or(u64::MAX);
        let past = |driver: &mut Self, what: &str, at: u64| {
            if at > fed {
                driver.note(
                    step,
                    Invariant::PositionRegressed,
                    format!("{what} at {at} past the {fed} fed in epoch {epoch}"),
                );
            }
        };
        match *observation {
            Observation::Window { index, .. } => {
                if let Some(last) = self.reference.last_window
                    && index <= last
                {
                    self.note(
                        step,
                        Invariant::PositionRegressed,
                        format!("window {index} after {last} in one epoch"),
                    );
                }
                self.reference.last_window = Some(index);
                past(
                    self,
                    "window end",
                    index.saturating_add(1).saturating_mul(span),
                );
            }
            Observation::VoiceStarted { at_sample } => {
                self.reference.voiced = true;
                past(self, "voice start", at_sample);
            }
            Observation::VoiceEnded { at_sample, .. } => {
                self.reference.voiced = false;
                past(self, "voice end", at_sample);
            }
            Observation::SilenceElapsed { at_sample } => past(self, "silence", at_sample),
            Observation::Reset { cause } => {
                if self.reference.voiced {
                    self.note(
                        step,
                        Invariant::VoiceLatchedAcrossReset,
                        format!("{cause:?} announced with voice still open"),
                    );
                }
                self.reference.voiced = false;
                self.reference.epoch = self.reference.epoch.saturating_add(1);
                self.reference.last_window = None;
            }
            Observation::Lost { count } => self.note(
                step,
                Invariant::QueueOverCapacity,
                format!(
                    "the lane with {REFERENCE_CAPACITY} slots lost {count} observations, so no \
                     position below can be attributed to an epoch"
                ),
            ),
            _ => {}
        }
    }

    /// Drain the subject and check every promise its observations make.
    fn drain(&mut self, step: usize) {
        let window = self.subject.analyzer.window_samples();
        let drained: Vec<Observation> = self.subject.analyzer.drain().collect();
        self.observations = self
            .observations
            .saturating_add(u64::try_from(drained.len()).unwrap_or(0));
        self.trace.push(format!(
            "step {step}: drained {} observations",
            drained.len()
        ));
        for observation in drained {
            self.check_observation(step, &observation, window);
            let reporter = &mut self.subject.reporter;
            if let Some(report) = reporter.observe(&observation, window) {
                self.check_report_domain(step, &report);
            }
        }
    }

    /// What a *lossy* stream still promises.
    ///
    /// Nothing here reads a position: §8.3 may have folded the `Reset` that opened an epoch into a
    /// counted marker, and a position measured against a guessed epoch is an oracle reporting its
    /// own defect. Everything below is a fact of one observation, true whatever the queue did.
    fn check_observation(&mut self, step: usize, observation: &Observation, window: u32) {
        match *observation {
            Observation::Window { .. } => {
                self.check_window(step, observation, window);
            }
            Observation::ThresholdUpdated {
                activation_amplitude,
                observed_floor,
                ..
            } => {
                let calibration = self.subject.analyzer.thresholds().calibration();
                if let Some(calibration) = calibration
                    && (activation_amplitude < calibration.floor_amplitude()
                        || activation_amplitude > calibration.ceiling_amplitude())
                {
                    self.note(
                        step,
                        Invariant::ThresholdOutOfBounds,
                        format!("announced {activation_amplitude}"),
                    );
                }
                if !(0..=32_768).contains(&observed_floor) {
                    self.note(
                        step,
                        Invariant::AccumulatorOutOfDomain,
                        format!("observed floor {observed_floor} outside 0..=32768"),
                    );
                }
            }
            // The loss marker is what §8.3 promises instead of growth: a counted absence is the
            // healthy outcome of backpressure, not a finding. Anything a later revision of the
            // observation set adds is not this harness's to reinterpret.
            _ => {}
        }
    }

    /// One completed window: where it sits, what its accumulators say, and whether calibration
    /// stayed inside the interval §12.11 promises it cannot leave.
    fn check_window(&mut self, step: usize, observation: &Observation, window: u32) {
        let Observation::Window {
            peak,
            sum,
            energy,
            clipped,
            clipping,
            impulsive,
            active,
            dc_offset,
            silent,
            ..
        } = *observation
        else {
            return;
        };
        self.check_accumulators(step, window, peak, sum, energy, clipped);
        self.check_predicates(
            step, window, peak, sum, energy, clipped, clipping, impulsive, active, dc_offset,
            silent,
        );
    }

    /// §5.2: every reported accumulator is inside the width proof's range for its window.
    fn check_accumulators(
        &mut self,
        step: usize,
        window: u32,
        peak: i32,
        sum: i64,
        energy: i64,
        clipped: u32,
    ) {
        let width = i64::from(window);
        let mut wrong = Vec::new();
        if !(0..=32_768).contains(&peak) {
            wrong.push(format!("peak {peak}"));
        }
        if clipped > window {
            wrong.push(format!("clipped {clipped} of {window}"));
        }
        if energy < 0 || energy > width.saturating_mul(1 << 30) {
            wrong.push(format!("energy {energy}"));
        }
        if sum.saturating_abs() > width.saturating_mul(32_768) {
            wrong.push(format!("sum {sum}"));
        }
        let deviation = window_deviation(window, sum, energy);
        if !(0..=32_768).contains(&deviation) {
            wrong.push(format!("deviation {deviation}"));
        }
        if !wrong.is_empty() {
            self.note(
                step,
                Invariant::AccumulatorOutOfDomain,
                format!("over {window} samples: {}", wrong.join(", ")),
            );
        }
    }

    /// §5.3: every reported fact is what its own accumulators say it is.
    ///
    /// `active` is checked against the *floor*, not against the value in force: the threshold moves
    /// between windows and a drain reports what it measured, not what it is measuring now. §12.11's
    /// first clause is the strongest thing that stays true whatever the threshold did, and the
    /// reference lane is where the exact answer comes from.
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    fn check_predicates(
        &mut self,
        step: usize,
        window: u32,
        peak: i32,
        sum: i64,
        energy: i64,
        clipped: u32,
        clipping: bool,
        impulsive: bool,
        active: bool,
        dc_offset: bool,
        silent: bool,
    ) {
        let profile = self.subject.analyzer.profile();
        let width = i64::from(window);
        let peak = i64::from(peak);
        let mut wrong = Vec::new();
        if clipping != (clipped >= profile.clip_samples()) {
            wrong.push(format!("clipping {clipping} from {clipped} clipped"));
        }
        let expect_impulsive = peak >= i64::from(profile.impulse_amplitude())
            && energy < peak.saturating_mul(peak).saturating_mul(2);
        if impulsive != expect_impulsive {
            wrong.push(format!("impulsive {impulsive} from peak {peak}"));
        }
        if dc_offset != (sum.saturating_abs() >= i64::from(profile.dc_amplitude()) * width) {
            wrong.push(format!("dc_offset {dc_offset} from sum {sum}"));
        }
        if silent != (peak < i64::from(profile.silence_amplitude())) {
            wrong.push(format!("silent {silent} from peak {peak}"));
        }
        if active && impulsive {
            wrong.push("active and impulsive at once".to_owned());
        }
        // §12.11's first clause, decided from this window's own accumulators rather than from a
        // second analyser: `active` at any reachable threshold implies `active` at the declared
        // floor, because the threshold never leaves `[floor, ceiling]` (§12.3, §12.5). No epoch and
        // no index are needed, so it survives a queue that coalesced the window before it.
        if active && let Some(calibration) = profile.calibration() {
            let floor = i64::from(calibration.floor_amplitude());
            let variance = width
                .saturating_mul(energy)
                .saturating_sub(sum.saturating_mul(sum));
            if variance
                < floor
                    .saturating_mul(floor)
                    .saturating_mul(width)
                    .saturating_mul(width)
            {
                self.note(
                    step,
                    Invariant::MoreSensitiveThanFloor,
                    format!(
                        "a window active under calibration has variance numerator {variance}, \
                         below the {floor} floor's own threshold"
                    ),
                );
            }
        }
        if !wrong.is_empty() {
            self.note(
                step,
                Invariant::PredicateDisagrees,
                format!("over {window} samples: {}", wrong.join(", ")),
            );
        }
    }

    /// `M-59`: a report never names coverage its epoch does not have.
    ///
    /// Only the reference lane may be asked this. A reducer fed a stream §8.3 coalesced discards
    /// the period in progress and carries on — which is correct, and which also means the epoch it
    /// is counting in is the epoch of the `Reset` observations that survived, not of the ones the
    /// analyser announced.
    fn check_report(&mut self, step: usize, report: &SignalObservation) {
        let SignalObservation::Report(report) = report else {
            return;
        };
        let fed = self
            .reference
            .epoch_samples
            .get(self.reference.epoch)
            .copied()
            .unwrap_or(u64::MAX);
        if report.at_sample.saturating_add(report.samples) > fed {
            self.note(
                step,
                Invariant::ReportOverclaimsCoverage,
                format!(
                    "report covers {}..{} of an epoch with {fed} samples",
                    report.at_sample,
                    report.at_sample.saturating_add(report.samples)
                ),
            );
        }
        self.check_report_domain(step, &SignalObservation::Report(*report));
    }

    /// What a report says about *itself*, which stays true on a lossy stream.
    fn check_report_domain(&mut self, step: usize, report: &SignalObservation) {
        let SignalObservation::Report(report) = report else {
            return;
        };
        let expected = self.subject.reporter.profile().windows_per_report();
        if report.peak > 32_768 || report.energy < 0 || report.windows != expected {
            self.note(
                step,
                Invariant::AccumulatorOutOfDomain,
                format!(
                    "report peak {} energy {} over {} windows, expected {expected}",
                    report.peak, report.energy, report.windows
                ),
            );
        }
        if report.samples
            != u64::from(report.windows) * u64::from(self.subject.analyzer.window_samples())
        {
            self.note(
                step,
                Invariant::ReportOverclaimsCoverage,
                format!(
                    "report of {} windows names {} samples",
                    report.windows, report.samples
                ),
            );
        }
    }

    fn finish(self) -> Run {
        Run {
            trace: self.trace,
            violations: self.violations,
            observations: self.observations,
            accepted: self.accepted,
            refused: self.refused,
        }
    }
}

/// Everything §7.3 says a refusal may not move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Snapshot {
    rate: u32,
    window: u32,
    activation: i32,
    queued: usize,
    voiced: bool,
}

impl Snapshot {
    fn of(analyzer: &AudioAnalyzer) -> Self {
        Self {
            rate: analyzer.sample_rate(),
            window: analyzer.window_samples(),
            activation: analyzer.activation_amplitude(),
            queued: analyzer.queued(),
            voiced: analyzer.is_voiced(),
        }
    }
}

/// §4's exact conversion, restated here so the oracle does not read the answer off the subject.
fn samples_for(duration_ms: u32, rate: u32) -> u64 {
    (u64::from(duration_ms) * u64::from(rate)).div_ceil(1_000)
}

/// Fold a fuzzer byte onto a table entry. Total for a non-empty table.
fn pick<T: Copy>(table: &[T], byte: u8) -> Option<T> {
    if table.is_empty() {
        return None;
    }
    table.get(usize::from(byte) % table.len()).copied()
}

// ---- the seed corpus ----

/// One named program, committed as a fuzzer seed.
#[derive(Debug, Clone)]
pub struct Seed {
    /// The file name it is written under, and what it is about.
    pub name: &'static str,
    /// The program itself.
    pub program: Program,
}

/// Where the committed seeds live, from the repository root.
pub const CORPUS_PATH: &str = "crates/sipx-testkit/corpus/call-audio-sequences";

/// The absolute path of the committed seed corpus.
#[must_use]
pub fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/call-audio-sequences")
}

/// Write every seed into [`corpus_dir`], returning how many files were written.
///
/// # Errors
///
/// Whatever creating the directory or writing a file returned.
pub fn write_corpus() -> std::io::Result<usize> {
    let dir = corpus_dir();
    std::fs::create_dir_all(&dir)?;
    let seeds = seeds();
    for seed in &seeds {
        std::fs::write(dir.join(seed.name), seed.program.encode())?;
    }
    Ok(seeds.len())
}

/// The adversarial corpus: one program per shape `M-61`'s Acceptance names.
///
/// A fuzzer starting from noise spends its budget learning that a four-byte record is an event.
/// Starting from these it spends it on the arithmetic, exactly as the RFC 4475 archive seeds the
/// parser targets.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn seeds() -> Vec<Seed> {
    let frame = |pattern: Pattern, amplitude: u8, samples: u8| Event::Frame {
        pattern,
        amplitude,
        samples,
    };
    // Index 6 of `FRAME_SAMPLES` is 160: one whole window of the reference profile.
    let window = 6u8;
    let mut seeds = Vec::new();
    let mut push = |name: &'static str, events: Vec<Event>| {
        seeds.push(Seed {
            name,
            program: Program { events },
        });
    };

    push(
        "extreme-amplitudes",
        (0..8)
            .flat_map(|_| [frame(Pattern::ExtremeAlternating, 11, window), Event::Drain])
            .collect(),
    );
    push(
        "impulse-storm",
        (0..12)
            .map(|index| {
                frame(
                    Pattern::Impulse,
                    11,
                    if index % 3 == 0 { 5 } else { window },
                )
            })
            .chain([Event::Drain])
            .collect(),
    );
    push(
        "impulse-train-under-modulation",
        (0..10)
            .flat_map(|_| [frame(Pattern::ImpulseTrain, 8, window), Event::Drain])
            .collect(),
    );
    push(
        "dc-bias",
        (0..10)
            .flat_map(|_| {
                [
                    frame(Pattern::Constant, 5, window),
                    frame(Pattern::NegativeConstant, 5, window),
                    Event::Drain,
                ]
            })
            .collect(),
    );
    push(
        "stuck-full-scale",
        (0..10)
            .flat_map(|_| [frame(Pattern::Constant, 11, window), Event::Drain])
            .collect(),
    );
    push(
        "alternating-full-swing",
        (0..10)
            .flat_map(|_| [frame(Pattern::Alternating, 8, window), Event::Drain])
            .collect(),
    );
    push(
        "arbitrary-chunking",
        // The same audio at 1, 2, 7, 40, 159, 160, 161 and 320 samples a frame: window boundaries
        // fall inside frames, across frames and exactly on them.
        (0..9u8)
            .flat_map(|samples| [frame(Pattern::Alternating, 8, samples), Event::Drain])
            .collect(),
    );
    push(
        "long-silence",
        (0..40)
            .map(|_| frame(Pattern::Zero, 0, window))
            .chain([Event::Drain])
            .collect(),
    );
    push(
        "permanent-activity",
        (0..40)
            .map(|_| frame(Pattern::Alternating, 8, window))
            .chain([Event::Drain])
            .collect(),
    );
    push(
        "event-backpressure",
        // Forty windows and one drain: §8.3's coalescing is the whole point, and the `Lost` marker
        // is what a bounded queue says instead of growing.
        (0..40)
            .map(|_| frame(Pattern::Noise, 8, window))
            .chain([Event::Drain])
            .collect(),
    );
    push(
        "sequence-gaps",
        (0..8)
            .flat_map(|index| {
                [
                    frame(Pattern::Alternating, 8, window),
                    Event::Gap {
                        skip: index,
                        pattern: Pattern::Noise,
                    },
                    Event::Drain,
                ]
            })
            .collect(),
    );
    push(
        "replayed-sequences",
        (0..8)
            .flat_map(|index| {
                [
                    frame(Pattern::Alternating, 8, window),
                    Event::Replay {
                        back: index,
                        flagged: index % 2 == 0,
                    },
                    Event::Drain,
                ]
            })
            .collect(),
    );
    push(
        "declared-breaks",
        (0..3)
            .flat_map(|_| {
                (0..3u8).flat_map(|kind| {
                    [
                        frame(Pattern::Alternating, 8, window),
                        Event::Broken {
                            kind,
                            pattern: Pattern::Noise,
                            amplitude: 8,
                        },
                        Event::Drain,
                    ]
                })
            })
            .collect(),
    );
    push(
        "format-churn",
        (0..12u8)
            .flat_map(|rate| {
                [
                    Event::DeclareFormat { rate },
                    frame(Pattern::Alternating, 8, window),
                    Event::Drain,
                ]
            })
            .collect(),
    );
    push(
        "refused-formats",
        // Rates 0 and 384,001 sit at indices 0 and 11 of `RATES`; neither may half-apply, and the
        // frames between them must be measured against the format still in force.
        vec![
            frame(Pattern::Alternating, 8, window),
            Event::DeclareFormat { rate: 0 },
            frame(Pattern::Alternating, 8, window),
            Event::DeclareFormat { rate: 11 },
            frame(Pattern::Alternating, 8, window),
            Event::Drain,
        ],
    );
    push(
        "every-source-format",
        // Every `(rate, encoding)` the application boundary supports, converted into the analyser's
        // declared format by the boundary itself.
        (0..12u8)
            .flat_map(|rate| {
                (0..2u8).flat_map(move |encoding| {
                    [
                        Event::SourceFormat { rate, encoding },
                        frame(Pattern::Alternating, 8, window),
                        frame(Pattern::Noise, 10, window),
                        Event::Drain,
                    ]
                })
            })
            .collect(),
    );
    push(
        "empty-and-oversized-frames",
        // Indices 0 and 11 of `FRAME_SAMPLES`: no samples at all, and one past the §3.3 ceiling.
        vec![
            frame(Pattern::Alternating, 8, 0),
            frame(Pattern::Alternating, 8, 11),
            frame(Pattern::Alternating, 8, window),
            Event::Drain,
        ],
    );
    push(
        "reset-storm",
        (0..12)
            .flat_map(|_| {
                [
                    frame(Pattern::Alternating, 8, window),
                    Event::Reset,
                    Event::Drain,
                ]
            })
            .collect(),
    );
    push(
        "calibration-ramp-then-cut",
        // Long enough to move the threshold under §12.5, then cut: §12.8 preserves the amplitude
        // and re-arms the warm-up, and no window after the cut may be active under a threshold the
        // floor does not also admit.
        (0..30)
            .map(|_| frame(Pattern::Zero, 0, window))
            .chain([Event::Reset])
            .chain((0..20).map(|_| frame(Pattern::Alternating, 5, window)))
            .chain([Event::Drain])
            .collect(),
    );
    push(
        "step-and-ramp",
        (0..8)
            .flat_map(|_| {
                [
                    frame(Pattern::Step, 9, window),
                    frame(Pattern::Ramp, 9, window),
                    Event::Drain,
                ]
            })
            .collect(),
    );

    seeds
}
