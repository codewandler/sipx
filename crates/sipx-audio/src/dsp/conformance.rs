//! The conformance harness of `docs/specs/custom-call-dsp.md` §11.
//!
//! A capability declaration is a set of claims, and this is where they are checked. The harness
//! accepts **any** [`FrameProcessor`] — built-in or application-supplied, from any crate — through
//! a factory that produces fresh instances, because half the checks need two instances that have
//! seen nothing.
//!
//! Two properties matter more than the check list. Every declared check appears in every report: a
//! check the harness could not prove is reported [`CheckStatus::Unproven`] with a stated reason and
//! never omitted and never passed, because a report whose length depends on what it managed to run
//! cannot be compared to yesterday's. And the harness is shown *failing*: §11.3 makes it normative
//! that the suite around this module contains, for each invariant, a fixture that violates exactly
//! that invariant, because a harness that has only ever seen well-behaved processors has proved
//! that it terminates.
//!
//! This module is a test tool and not real-time code: it allocates freely, which is exactly what
//! the processors it measures may not do.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::analysis::{AudioDirection, DiscontinuityKind};

use super::contract::{
    DspCapability, DspFrame, DspObservation, DspResetCause, FrameProcessor, FrameSink,
    LengthPolicy, MAX_CHANNELS, Parameter, ParameterDomain, ParameterValue, ProcessError,
    RateSupport, Scratch, StreamFormat,
};

/// The declaration is internally admissible (§5).
pub const CHECK_CAPABILITY: &str = "DSP-K1";
/// A format outside the declaration is refused typed, and the processor still runs afterwards.
pub const CHECK_FORMAT_REFUSAL: &str = "DSP-K2";
/// Every frame refusal is typed and mutates nothing.
pub const CHECK_FRAME_REFUSAL: &str = "DSP-K3";
/// A refused parameter set leaves the previous one in force.
pub const CHECK_PARAMETER_REFUSAL: &str = "DSP-K4";
/// Two fresh instances fed the same frames produce identical output.
pub const CHECK_DETERMINISM: &str = "DSP-K5";
/// One stream produces the same samples however the caller cut it into frames.
pub const CHECK_CHUNK_BOUNDARY: &str = "DSP-K6";
/// A reset discards retained audio and restarts measurement.
pub const CHECK_RESET: &str = "DSP-K7";
/// Cancellation is terminal, idempotent and releases everything.
pub const CHECK_CANCELLATION: &str = "DSP-K8";
/// The workspace bounds hold and the declared scratch is what was actually asked for.
pub const CHECK_ALLOCATION: &str = "DSP-K9";
/// A flagged frame restarts state before its own samples.
pub const CHECK_DISCONTINUITY: &str = "DSP-K10";
/// The declared length policy is what the processor does.
pub const CHECK_LENGTH: &str = "DSP-K11";
/// Hostile amplitudes produce a typed result and never a panic.
pub const CHECK_EXTREMES: &str = "DSP-K12";

/// Every check this harness runs, in report order.
///
/// A downstream story asserting on one of these is asserting on a name, so the list and the strings
/// in it are part of the contract.
pub const CHECKS: &[&str] = &[
    CHECK_CAPABILITY,
    CHECK_FORMAT_REFUSAL,
    CHECK_FRAME_REFUSAL,
    CHECK_PARAMETER_REFUSAL,
    CHECK_DETERMINISM,
    CHECK_CHUNK_BOUNDARY,
    CHECK_RESET,
    CHECK_CANCELLATION,
    CHECK_ALLOCATION,
    CHECK_DISCONTINUITY,
    CHECK_LENGTH,
    CHECK_EXTREMES,
];

/// How many observations one frame's queue holds during a conformance run.
const OBSERVATION_CAPACITY: u32 = 16;

/// The largest frame, in positions, the harness will build.
const REFERENCE_FRAME_POSITIONS: usize = 16;

/// The two workspace fill patterns §11.2 poisons a run with.
const POISON: [i16; 2] = [0x5A5A, -0x5A5B];

/// What one check concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CheckStatus {
    /// The processor satisfied the invariant on the harness's inputs.
    Passed,
    /// The processor violated the invariant, and the outcome's detail says how.
    Failed,
    /// The harness could not prove the invariant here, and the detail says why. Never a pass:
    /// reporting an unprovable claim as satisfied is the failure mode this status exists to make
    /// impossible.
    Unproven,
}

/// One check's outcome.
#[derive(Debug, Clone)]
pub struct CheckOutcome {
    id: &'static str,
    name: &'static str,
    status: CheckStatus,
    detail: String,
}

impl CheckOutcome {
    /// The stable identifier §11.1 names.
    #[must_use]
    pub fn id(&self) -> &'static str {
        self.id
    }

    /// A short human name for the invariant.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Whether the invariant held, failed, or could not be proved.
    #[must_use]
    pub fn status(&self) -> CheckStatus {
        self.status
    }

    /// What the harness measured, in words. Never empty.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for CheckOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = match self.status {
            CheckStatus::Passed => "pass",
            CheckStatus::Failed => "FAIL",
            CheckStatus::Unproven => "unproven",
        };
        write!(f, "{} {} [{status}] {}", self.id, self.name, self.detail)
    }
}

/// Everything one conformance run concluded.
#[derive(Debug, Clone)]
pub struct ConformanceReport {
    checks: Vec<CheckOutcome>,
}

impl ConformanceReport {
    /// Every check, in [`CHECKS`] order.
    #[must_use]
    pub fn checks(&self) -> &[CheckOutcome] {
        &self.checks
    }

    /// Whether no check failed.
    ///
    /// An [`CheckStatus::Unproven`] outcome does not fail a run — it is a claim the harness could
    /// not reach — but it is always listed, and [`Self::unproven`] is how a caller that requires a
    /// complete proof finds out that it did not get one.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.failures().next().is_none()
    }

    /// The checks that failed.
    pub fn failures(&self) -> impl Iterator<Item = &CheckOutcome> {
        self.checks
            .iter()
            .filter(|check| check.status == CheckStatus::Failed)
    }

    /// The checks the harness could not prove.
    pub fn unproven(&self) -> impl Iterator<Item = &CheckOutcome> {
        self.checks
            .iter()
            .filter(|check| check.status == CheckStatus::Unproven)
    }

    /// Whether the named check failed.
    #[must_use]
    pub fn failed(&self, id: &str) -> bool {
        self.failures().any(|check| check.id == id)
    }
}

impl fmt::Display for ConformanceReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "conformance report:")?;
        for check in &self.checks {
            writeln!(f, "  {check}")?;
        }
        Ok(())
    }
}

/// A conformance run's configuration.
///
/// The stream length and frame sizes are derived from the processor's own declaration rather than
/// configured, so that a processor with a small frame ceiling is measured against frames it accepts
/// instead of being failed for a choice the harness made.
#[derive(Debug, Clone, Copy)]
pub struct Conformance {
    direction: AudioDirection,
    format: StreamFormat,
}

impl Default for Conformance {
    /// §12.1's reference stream `D8`: inbound, 8,000 Hz, one channel.
    fn default() -> Self {
        Self {
            direction: AudioDirection::Inbound,
            format: StreamFormat::default(),
        }
    }
}

impl Conformance {
    /// A run against §12.1's reference stream.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Run against a different stream format.
    #[must_use]
    pub fn with_format(mut self, format: StreamFormat) -> Self {
        self.format = format;
        self
    }

    /// Run against the other call direction.
    #[must_use]
    pub fn with_direction(mut self, direction: AudioDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Run every check in [`CHECKS`] against processors this factory builds.
    ///
    /// The factory is called once per check that needs a fresh instance. A panic anywhere in a run
    /// is caught and reported as that check's failure rather than taking the harness down: a
    /// processor that panics is a finding, and a harness that dies with it produces no finding at
    /// all. That requires the unwinding panic strategy; under `panic = "abort"` this cannot be
    /// promised and is not claimed.
    pub fn run<P, F>(&self, mut factory: F) -> ConformanceReport
    where
        P: FrameProcessor,
        F: FnMut() -> P,
    {
        let Ok(capability) = catch_unwind(AssertUnwindSafe(|| factory().capability())) else {
            return ConformanceReport {
                checks: CHECKS
                    .iter()
                    .map(|id| {
                        outcome(
                            id,
                            "declaration",
                            CheckStatus::Failed,
                            "the processor panicked while declaring its capability".to_owned(),
                        )
                    })
                    .collect(),
            };
        };
        let channels = usize::from(self.format.channels());
        let positions = frame_positions(&capability, channels);
        let wave = signal(self.format, positions.saturating_mul(3));

        // Sequential rather than a table of closures, so each check's mutable borrow of the
        // factory ends before the next one starts.
        let mut checks = Vec::with_capacity(CHECKS.len());
        checks.push(guarded(CHECK_CAPABILITY, "capability", || {
            check_capability(&mut factory)
        }));
        checks.push(guarded(CHECK_FORMAT_REFUSAL, "format refusal", || {
            self.check_format_refusal(&mut factory, &wave, positions)
        }));
        checks.push(guarded(CHECK_FRAME_REFUSAL, "frame refusal", || {
            self.check_frame_refusal(&mut factory, &wave, positions)
        }));
        checks.push(guarded(
            CHECK_PARAMETER_REFUSAL,
            "parameter refusal",
            || self.check_parameter_refusal(&mut factory, &wave, positions),
        ));
        checks.push(guarded(CHECK_DETERMINISM, "determinism", || {
            self.check_determinism(&mut factory, &wave, positions)
        }));
        checks.push(guarded(CHECK_CHUNK_BOUNDARY, "chunk boundary", || {
            self.check_chunk_boundary(&mut factory, &wave, positions)
        }));
        checks.push(guarded(CHECK_RESET, "reset", || {
            self.check_reset(&mut factory, &wave, positions)
        }));
        checks.push(guarded(CHECK_CANCELLATION, "cancellation", || {
            self.check_cancellation(&mut factory, &wave, positions)
        }));
        checks.push(guarded(CHECK_ALLOCATION, "allocation", || {
            self.check_allocation(&mut factory, &wave, positions)
        }));
        checks.push(guarded(CHECK_DISCONTINUITY, "discontinuity", || {
            self.check_discontinuity(&mut factory, &wave, positions)
        }));
        checks.push(guarded(CHECK_LENGTH, "length", || {
            self.check_length(&mut factory, &wave, positions)
        }));
        checks.push(guarded(CHECK_EXTREMES, "extremes", || {
            self.check_extremes(&mut factory, positions)
        }));
        ConformanceReport { checks }
    }

    // -------------------------------------------------------------------------- checks ----

    fn check_format_refusal<P: FrameProcessor, F: FnMut() -> P>(
        &self,
        factory: &mut F,
        signal: &[i16],
        frame_positions: usize,
    ) -> CheckOutcome {
        let capability = factory().capability();
        let reference = match self.reference(factory, signal, frame_positions) {
            Ok(drive) => drive,
            Err(error) => {
                return setup_failure(CHECK_FORMAT_REFUSAL, "format refusal", &error);
            }
        };

        let unaccepted_rate = match capability.rates() {
            RateSupport::Any => None,
            RateSupport::Exactly(_) => (1..=48u32)
                .map(|step| step * 8_000)
                .find(|rate| !capability.rates().accepts(*rate)),
        };
        let unaccepted_channels =
            (1..=MAX_CHANNELS).find(|count| !capability.channels().contains(count));

        let mut tried = Vec::new();
        let mut processor = factory();
        let rate_case = unaccepted_rate
            .and_then(|rate| StreamFormat::new(rate, self.format.channels()).ok())
            .zip(unaccepted_rate);
        if let Some((format, rate)) = rate_case {
            if processor.prepare(self.direction, format).is_ok() {
                return failed(
                    CHECK_FORMAT_REFUSAL,
                    "format refusal",
                    format!("{rate} Hz is outside the declaration and was accepted anyway"),
                );
            }
            tried.push(format!("{rate} Hz"));
        }
        let channel_case = unaccepted_channels
            .and_then(|count| StreamFormat::new(self.format.sample_rate(), count).ok())
            .zip(unaccepted_channels);
        if let Some((format, count)) = channel_case {
            if processor.prepare(self.direction, format).is_ok() {
                return failed(
                    CHECK_FORMAT_REFUSAL,
                    "format refusal",
                    format!("{count} channels are outside the declaration and were accepted"),
                );
            }
            tried.push(format!("{count} channels"));
        }

        if tried.is_empty() {
            return unproven(
                CHECK_FORMAT_REFUSAL,
                "format refusal",
                "this processor accepts every rate and every channel count the contract admits, \
                 so there is no admissible format for it to refuse"
                    .to_owned(),
            );
        }

        // The refusals must have changed nothing: the same instance, prepared on an accepted
        // format, has to reproduce a fresh instance's stream exactly.
        if let Err(error) = processor.prepare(self.direction, self.format) {
            return failed(
                CHECK_FORMAT_REFUSAL,
                "format refusal",
                format!("a refused format left the processor unable to prepare: {error}"),
            );
        }
        let mut drive = Drive::new(self.direction, self.format, POISON[0]);
        if let Err(error) = drive_stream(&mut processor, &mut drive, signal, frame_positions) {
            return failed(
                CHECK_FORMAT_REFUSAL,
                "format refusal",
                format!("a refused format left the processor unable to run: {error}"),
            );
        }
        if drive.samples != reference.samples {
            return failed(
                CHECK_FORMAT_REFUSAL,
                "format refusal",
                "a refused format changed what the processor produced afterwards".to_owned(),
            );
        }
        passed(
            CHECK_FORMAT_REFUSAL,
            "format refusal",
            format!("refused {} and continued unchanged", tried.join(", ")),
        )
    }

    fn check_frame_refusal<P: FrameProcessor, F: FnMut() -> P>(
        &self,
        factory: &mut F,
        signal: &[i16],
        frame_positions: usize,
    ) -> CheckOutcome {
        let capability = factory().capability();
        let reference = match self.reference(factory, signal, frame_positions) {
            Ok(drive) => drive,
            Err(error) => return setup_failure(CHECK_FRAME_REFUSAL, "frame refusal", &error),
        };

        let channels = usize::from(self.format.channels()).max(1);
        let step = frame_positions.saturating_mul(channels);
        let (Some(head), Some(tail)) = (signal.get(..step), signal.get(step..)) else {
            return setup_failure(CHECK_FRAME_REFUSAL, "frame refusal", "the signal is short");
        };
        // Where the stream stands after one frame, and therefore what a contiguous frame's
        // position would be.
        let boundary = u64::try_from(frame_positions).unwrap_or(u64::MAX);

        // Every refusal §8.3 names that is a property of the frame rather than of the processor's
        // own arithmetic. The buffers outlive the frames that borrow them.
        let empty: [i16; 0] = [];
        let oversize = vec![0i16; usize_of(capability.max_frame_samples()).saturating_add(1)];
        let partial = vec![0i16; step.saturating_sub(1)];
        let cases = self.refusal_cases(head, boundary, &empty, &oversize, &partial);

        // One refusal at a time, each spliced into its own replay of the reference stream. A batch
        // of refusals can hide a mutation that a later one happens to undo, and that is precisely
        // the state change this check exists to find.
        for (what, frame) in &cases {
            let mut processor = factory();
            if let Err(error) = processor.prepare(self.direction, self.format) {
                return setup_failure(CHECK_FRAME_REFUSAL, "frame refusal", &format!("{error}"));
            }
            let mut drive = Drive::new(self.direction, self.format, POISON[0]);
            if let Err(error) = feed(&mut processor, &mut drive, head, 0, None) {
                return setup_failure(CHECK_FRAME_REFUSAL, "frame refusal", &format!("{error}"));
            }
            match refuse(&mut processor, &capability, frame) {
                Refusal::Accepted => {
                    return failed(
                        CHECK_FRAME_REFUSAL,
                        "frame refusal",
                        format!("{what} was accepted"),
                    );
                }
                Refusal::Wrote => {
                    return failed(
                        CHECK_FRAME_REFUSAL,
                        "frame refusal",
                        format!("{what} was refused and still wrote to the sink"),
                    );
                }
                Refusal::Refused => {}
            }
            if let Err(error) = drive_stream(&mut processor, &mut drive, tail, frame_positions) {
                return failed(
                    CHECK_FRAME_REFUSAL,
                    "frame refusal",
                    format!("the stream did not survive {what}: {error}"),
                );
            }
            if drive.samples != reference.samples || drive.observations != reference.observations {
                return failed(
                    CHECK_FRAME_REFUSAL,
                    "frame refusal",
                    format!(
                        "{what} was refused and still mutated state: the rest of the stream came \
                         out different from a clean run"
                    ),
                );
            }
        }
        passed(
            CHECK_FRAME_REFUSAL,
            "frame refusal",
            format!(
                "{} refusals mutated nothing, each proved on its own replay",
                cases.len()
            ),
        )
    }

    /// §8.3's frame refusals, each as a frame to splice into a live stream.
    fn refusal_cases<'a>(
        &self,
        head: &'a [i16],
        boundary: u64,
        empty: &'a [i16],
        oversize: &'a [i16],
        partial: &'a [i16],
    ) -> Vec<(&'static str, DspFrame<'a>)> {
        let other_direction = match self.direction {
            AudioDirection::Inbound => AudioDirection::Outbound,
            AudioDirection::Outbound => AudioDirection::Inbound,
        };
        let other_rate = if self.format.sample_rate() == 16_000 {
            8_000
        } else {
            16_000
        };
        let mut cases: Vec<(&'static str, DspFrame<'a>)> = vec![
            (
                "an empty frame",
                DspFrame::new(self.direction, self.format, boundary, empty),
            ),
            (
                "a frame past the declared ceiling",
                DspFrame::new(self.direction, self.format, boundary, oversize),
            ),
            (
                "a frame whose position is not contiguous",
                DspFrame::new(
                    self.direction,
                    self.format,
                    boundary.saturating_add(1),
                    head,
                ),
            ),
            (
                "a flagged frame that moves the position backwards",
                DspFrame::new(self.direction, self.format, 0, head)
                    .with_discontinuity(DiscontinuityKind::Loss),
            ),
            (
                "a frame carrying the other direction",
                DspFrame::new(other_direction, self.format, boundary, head),
            ),
        ];
        if self.format.channels() > 1 {
            cases.push((
                "a frame carrying a partial position",
                DspFrame::new(self.direction, self.format, boundary, partial),
            ));
        }
        if let Ok(format) = StreamFormat::new(other_rate, self.format.channels()) {
            cases.push((
                "a frame in an undeclared format",
                DspFrame::new(self.direction, format, boundary, head),
            ));
        }
        cases
    }

    fn check_parameter_refusal<P: FrameProcessor, F: FnMut() -> P>(
        &self,
        factory: &mut F,
        signal: &[i16],
        frame_positions: usize,
    ) -> CheckOutcome {
        let capability = factory().capability();
        let reference = match self.reference(factory, signal, frame_positions) {
            Ok(drive) => drive,
            Err(error) => {
                return setup_failure(CHECK_PARAMETER_REFUSAL, "parameter refusal", &error);
            }
        };

        let mut processor = factory();
        if let Err(error) = processor.prepare(self.direction, self.format) {
            return setup_failure(
                CHECK_PARAMETER_REFUSAL,
                "parameter refusal",
                &format!("{error}"),
            );
        }

        let mut offered = vec![Parameter::new(
            "__sipx_undeclared_parameter",
            ParameterValue::Flag(true),
        )];
        for spec in capability.parameters() {
            offered.push(Parameter::new(spec.id(), wrong_kind(spec.domain())));
            if let Some(value) = out_of_range(spec.domain()) {
                offered.push(Parameter::new(spec.id(), value));
            }
        }
        for parameter in &offered {
            if processor.configure(std::slice::from_ref(parameter)).is_ok() {
                return failed(
                    CHECK_PARAMETER_REFUSAL,
                    "parameter refusal",
                    format!("`{}` accepted an inadmissible value", parameter.id()),
                );
            }
        }

        let mut drive = Drive::new(self.direction, self.format, POISON[0]);
        if let Err(error) = drive_stream(&mut processor, &mut drive, signal, frame_positions) {
            return failed(
                CHECK_PARAMETER_REFUSAL,
                "parameter refusal",
                format!("a refused parameter left the processor unable to run: {error}"),
            );
        }
        if drive.samples != reference.samples {
            return failed(
                CHECK_PARAMETER_REFUSAL,
                "parameter refusal",
                "a refused parameter set changed the processor's behaviour".to_owned(),
            );
        }
        passed(
            CHECK_PARAMETER_REFUSAL,
            "parameter refusal",
            format!(
                "{} inadmissible assignments refused, previous state intact",
                offered.len()
            ),
        )
    }

    fn check_determinism<P: FrameProcessor, F: FnMut() -> P>(
        &self,
        factory: &mut F,
        signal: &[i16],
        frame_positions: usize,
    ) -> CheckOutcome {
        let mut runs = Vec::with_capacity(2);
        for poison in POISON {
            let mut processor = factory();
            if let Err(error) = processor.prepare(self.direction, self.format) {
                return setup_failure(CHECK_DETERMINISM, "determinism", &format!("{error}"));
            }
            let mut drive = Drive::new(self.direction, self.format, poison);
            if let Err(error) = drive_stream(&mut processor, &mut drive, signal, frame_positions) {
                return setup_failure(CHECK_DETERMINISM, "determinism", &format!("{error}"));
            }
            runs.push(drive);
        }
        let (Some(first), Some(second)) = (runs.first(), runs.get(1)) else {
            return setup_failure(CHECK_DETERMINISM, "determinism", "no runs");
        };
        if first.samples != second.samples {
            return failed(
                CHECK_DETERMINISM,
                "determinism",
                "two fresh instances fed identical frames produced different samples; the \
                 workspace was poisoned differently on each run, so reading unwritten scratch \
                 shows up here"
                    .to_owned(),
            );
        }
        if first.observations != second.observations {
            return failed(
                CHECK_DETERMINISM,
                "determinism",
                "two fresh instances fed identical frames produced different observations"
                    .to_owned(),
            );
        }
        passed(
            CHECK_DETERMINISM,
            "determinism",
            format!(
                "{} samples identical across two poisoned runs",
                first.samples.len()
            ),
        )
    }

    fn check_chunk_boundary<P: FrameProcessor, F: FnMut() -> P>(
        &self,
        factory: &mut F,
        signal: &[i16],
        frame_positions: usize,
    ) -> CheckOutcome {
        let capability = factory().capability();
        if let LengthPolicy::Bounded { max_positions } = capability.length() {
            return unproven(
                CHECK_CHUNK_BOUNDARY,
                "chunk boundary",
                format!(
                    "this processor declares `Bounded {{ max_positions: {max_positions} }}`, so \
                     re-cutting the stream legitimately changes its output shape and the harness \
                     has no equality to assert"
                ),
            );
        }
        let mut whole = Drive::new(self.direction, self.format, POISON[0]);
        let mut split = Drive::new(self.direction, self.format, POISON[0]);
        let mut coarse = factory();
        let mut fine = factory();
        for (processor, drive, positions) in [
            (&mut coarse, &mut whole, frame_positions),
            (&mut fine, &mut split, 1usize),
        ] {
            if let Err(error) = processor.prepare(self.direction, self.format) {
                return setup_failure(CHECK_CHUNK_BOUNDARY, "chunk boundary", &format!("{error}"));
            }
            if let Err(error) = drive_stream(processor, drive, signal, positions) {
                return setup_failure(CHECK_CHUNK_BOUNDARY, "chunk boundary", &format!("{error}"));
            }
        }
        let coarse_tail = flush_all(&mut coarse, self.format);
        let fine_tail = flush_all(&mut fine, self.format);
        if whole.samples != split.samples {
            return failed(
                CHECK_CHUNK_BOUNDARY,
                "chunk boundary",
                format!(
                    "the same stream in {frame_positions}-position frames and in 1-position \
                     frames produced different samples ({} against {})",
                    whole.samples.len(),
                    split.samples.len()
                ),
            );
        }
        if coarse_tail != fine_tail {
            return failed(
                CHECK_CHUNK_BOUNDARY,
                "chunk boundary",
                "the flushed tails differ between two cuttings of the same stream".to_owned(),
            );
        }
        passed(
            CHECK_CHUNK_BOUNDARY,
            "chunk boundary",
            format!(
                "{} samples and a {}-sample tail agree across two cuttings",
                whole.samples.len(),
                coarse_tail.len()
            ),
        )
    }

    fn check_reset<P: FrameProcessor, F: FnMut() -> P>(
        &self,
        factory: &mut F,
        signal: &[i16],
        frame_positions: usize,
    ) -> CheckOutcome {
        let channels = usize::from(self.format.channels());
        let step = frame_positions * channels;
        let (Some(head), Some(tail)) = (signal.get(..step), signal.get(step..)) else {
            return setup_failure(CHECK_RESET, "reset", "the signal is short");
        };

        let mut subject = factory();
        if let Err(error) = subject.prepare(self.direction, self.format) {
            return setup_failure(CHECK_RESET, "reset", &format!("{error}"));
        }
        let mut warm = Drive::new(self.direction, self.format, POISON[0]);
        if let Err(error) = feed(&mut subject, &mut warm, head, 0, None) {
            return setup_failure(CHECK_RESET, "reset", &format!("{error}"));
        }
        subject.reset(DspResetCause::Requested);
        if subject.retained() != 0 {
            return failed(
                CHECK_RESET,
                "reset",
                format!(
                    "{} positions were still retained after a reset; a reset discards retained \
                     audio rather than keeping it for an epoch that no longer exists",
                    subject.retained()
                ),
            );
        }
        let mut after = Drive::new(self.direction, self.format, POISON[0]);
        if let Err(error) = drive_stream(&mut subject, &mut after, tail, frame_positions) {
            return failed(
                CHECK_RESET,
                "reset",
                format!("the processor did not accept a new epoch after a reset: {error}"),
            );
        }

        let mut fresh = factory();
        if let Err(error) = fresh.prepare(self.direction, self.format) {
            return setup_failure(CHECK_RESET, "reset", &format!("{error}"));
        }
        let mut clean = Drive::new(self.direction, self.format, POISON[0]);
        if let Err(error) = drive_stream(&mut fresh, &mut clean, tail, frame_positions) {
            return setup_failure(CHECK_RESET, "reset", &format!("{error}"));
        }

        if after.samples != clean.samples {
            return failed(
                CHECK_RESET,
                "reset",
                "a stream processed after a reset differs from the same stream on a fresh \
                 instance, so sample memory survived the reset"
                    .to_owned(),
            );
        }
        passed(
            CHECK_RESET,
            "reset",
            format!(
                "a reset left nothing retained and reproduced a fresh instance over {} samples",
                clean.samples.len()
            ),
        )
    }

    fn check_cancellation<P: FrameProcessor, F: FnMut() -> P>(
        &self,
        factory: &mut F,
        signal: &[i16],
        frame_positions: usize,
    ) -> CheckOutcome {
        let capability = factory().capability();
        let mut processor = factory();
        if let Err(error) = processor.prepare(self.direction, self.format) {
            return setup_failure(CHECK_CANCELLATION, "cancellation", &format!("{error}"));
        }
        let mut drive = Drive::new(self.direction, self.format, POISON[0]);
        if let Err(error) = drive_stream(&mut processor, &mut drive, signal, frame_positions) {
            return setup_failure(CHECK_CANCELLATION, "cancellation", &format!("{error}"));
        }

        for round in 0..2 {
            processor.cancel();
            if processor.retained() != 0 {
                return failed(
                    CHECK_CANCELLATION,
                    "cancellation",
                    format!(
                        "{} positions were still retained after cancel #{}",
                        processor.retained(),
                        round + 1
                    ),
                );
            }
            let channels = usize::from(self.format.channels());
            let Some(chunk) = signal.get(..frame_positions * channels) else {
                return setup_failure(CHECK_CANCELLATION, "cancellation", "the signal is short");
            };
            let frame = DspFrame::new(self.direction, self.format, drive.position, chunk);
            match refuse(&mut processor, &capability, &frame) {
                Refusal::Accepted => {
                    return failed(
                        CHECK_CANCELLATION,
                        "cancellation",
                        "a frame was processed after cancellation".to_owned(),
                    );
                }
                Refusal::Wrote => {
                    return failed(
                        CHECK_CANCELLATION,
                        "cancellation",
                        "a frame refused after cancellation still wrote to the sink".to_owned(),
                    );
                }
                Refusal::Refused => {}
            }
            let mut output = vec![0i16; usize_of(capability.tail_positions()) * channels];
            let mut observations = Vec::with_capacity(usize_of(OBSERVATION_CAPACITY));
            let mut sink = FrameSink::new(&mut output, &mut observations, OBSERVATION_CAPACITY);
            if processor.flush(&mut sink).is_ok() {
                return failed(
                    CHECK_CANCELLATION,
                    "cancellation",
                    "a flush succeeded after cancellation".to_owned(),
                );
            }
        }
        passed(
            CHECK_CANCELLATION,
            "cancellation",
            "cancellation released everything, refused further work and was idempotent".to_owned(),
        )
    }

    fn check_allocation<P: FrameProcessor, F: FnMut() -> P>(
        &self,
        factory: &mut F,
        signal: &[i16],
        frame_positions: usize,
    ) -> CheckOutcome {
        let capability = factory().capability();
        let drive = match self.reference(factory, signal, frame_positions) {
            Ok(drive) => drive,
            Err(error) => return setup_failure(CHECK_ALLOCATION, "allocation", &error),
        };
        if drive.scratch_high_water > capability.scratch_samples() {
            return failed(
                CHECK_ALLOCATION,
                "allocation",
                format!(
                    "the processor asked for {} scratch samples against {} declared; the workspace \
                     bound held, and the declaration did not",
                    drive.scratch_high_water,
                    capability.scratch_samples()
                ),
            );
        }
        let inline = u64::try_from(std::mem::size_of::<P>()).unwrap_or(u64::MAX);
        if inline > capability.state_bytes() {
            return failed(
                CHECK_ALLOCATION,
                "allocation",
                format!(
                    "the processor's inline size is {inline} bytes against {} declared",
                    capability.state_bytes()
                ),
            );
        }
        unproven(
            CHECK_ALLOCATION,
            "allocation",
            format!(
                "the workspace bound held ({} of {} scratch samples requested, no sink overrun) \
                 and the inline state is {inline} of {} declared bytes. Heap growth inside the \
                 processor's own state is not observable here: `unsafe_code` is forbidden \
                 workspace-wide, so no counting allocator can be installed, and a figure that \
                 cannot be produced is not reported as one",
                drive.scratch_high_water,
                capability.scratch_samples(),
                capability.state_bytes()
            ),
        )
    }

    fn check_discontinuity<P: FrameProcessor, F: FnMut() -> P>(
        &self,
        factory: &mut F,
        signal: &[i16],
        frame_positions: usize,
    ) -> CheckOutcome {
        let channels = usize::from(self.format.channels());
        let step = frame_positions * channels;
        let (Some(head), Some(next)) = (signal.get(..step), signal.get(step..step * 2)) else {
            return setup_failure(CHECK_DISCONTINUITY, "discontinuity", "the signal is short");
        };

        let mut subject = factory();
        if let Err(error) = subject.prepare(self.direction, self.format) {
            return setup_failure(CHECK_DISCONTINUITY, "discontinuity", &format!("{error}"));
        }
        let mut drive = Drive::new(self.direction, self.format, POISON[0]);
        if let Err(error) = feed(&mut subject, &mut drive, head, 0, None) {
            return setup_failure(CHECK_DISCONTINUITY, "discontinuity", &format!("{error}"));
        }
        let boundary = drive.samples.len();
        if let Err(error) = feed(
            &mut subject,
            &mut drive,
            next,
            0,
            Some(DiscontinuityKind::Realign),
        ) {
            return failed(
                CHECK_DISCONTINUITY,
                "discontinuity",
                format!("a `Realign` frame at position 0 was refused: {error}"),
            );
        }
        let realigned = drive.samples.get(boundary..).unwrap_or(&[]).to_vec();

        let mut fresh = factory();
        if let Err(error) = fresh.prepare(self.direction, self.format) {
            return setup_failure(CHECK_DISCONTINUITY, "discontinuity", &format!("{error}"));
        }
        let mut clean = Drive::new(self.direction, self.format, POISON[0]);
        if let Err(error) = feed(&mut fresh, &mut clean, next, 0, None) {
            return setup_failure(CHECK_DISCONTINUITY, "discontinuity", &format!("{error}"));
        }
        if realigned != clean.samples {
            return failed(
                CHECK_DISCONTINUITY,
                "discontinuity",
                "a `Realign` frame did not produce what a fresh instance produces, so the reset \
                 did not run before the flagged frame's own samples"
                    .to_owned(),
            );
        }

        // A forward-skipping `Loss` continues the epoch rather than restarting it, and must be
        // accepted at the position the seam states.
        let skipped = drive.position.saturating_add(64);
        if let Err(error) = feed(
            &mut subject,
            &mut drive,
            next,
            skipped,
            Some(DiscontinuityKind::Loss),
        ) {
            return failed(
                CHECK_DISCONTINUITY,
                "discontinuity",
                format!("a flagged `Loss` frame skipping forward was refused: {error}"),
            );
        }
        passed(
            CHECK_DISCONTINUITY,
            "discontinuity",
            "a `Realign` reproduced a fresh instance and a forward `Loss` was accepted".to_owned(),
        )
    }

    fn check_length<P: FrameProcessor, F: FnMut() -> P>(
        &self,
        factory: &mut F,
        signal: &[i16],
        frame_positions: usize,
    ) -> CheckOutcome {
        let capability = factory().capability();
        let drive = match self.reference(factory, signal, frame_positions) {
            Ok(drive) => drive,
            Err(error) => return setup_failure(CHECK_LENGTH, "length", &error),
        };
        match drive.length_violation {
            Some(error) => failed(
                CHECK_LENGTH,
                "length",
                format!(
                    "{error} against a declared {:?} policy",
                    capability.length()
                ),
            ),
            None => passed(
                CHECK_LENGTH,
                "length",
                format!("every frame honoured {:?}", capability.length()),
            ),
        }
    }

    fn check_extremes<P: FrameProcessor, F: FnMut() -> P>(
        &self,
        factory: &mut F,
        frame_positions: usize,
    ) -> CheckOutcome {
        let channels = usize::from(self.format.channels());
        let samples = frame_positions * channels;
        let hostile: [(&'static str, Vec<i16>); 5] = [
            ("full scale positive", vec![i16::MAX; samples]),
            ("full scale negative", vec![i16::MIN; samples]),
            (
                "alternating full scale",
                (0..samples)
                    .map(|index| if index % 2 == 0 { i16::MAX } else { i16::MIN })
                    .collect(),
            ),
            ("a DC offset", vec![12_345i16; samples]),
            ("silence", vec![0i16; samples]),
        ];

        let mut processor = factory();
        if let Err(error) = processor.prepare(self.direction, self.format) {
            return setup_failure(CHECK_EXTREMES, "extremes", &format!("{error}"));
        }
        let mut drive = Drive::new(self.direction, self.format, POISON[0]);
        for (what, chunk) in &hostile {
            let position = drive.position;
            if let Err(error) = feed(&mut processor, &mut drive, chunk, position, None) {
                return failed(
                    CHECK_EXTREMES,
                    "extremes",
                    format!("{what} was refused with {error}"),
                );
            }
        }
        passed(
            CHECK_EXTREMES,
            "extremes",
            format!("{} hostile frames produced a typed result", hostile.len()),
        )
    }

    // -------------------------------------------------------------------------- helpers ----

    fn reference<P: FrameProcessor, F: FnMut() -> P>(
        &self,
        factory: &mut F,
        signal: &[i16],
        frame_positions: usize,
    ) -> Result<Drive, String> {
        let mut processor = factory();
        processor
            .prepare(self.direction, self.format)
            .map_err(|error| format!("{error}"))?;
        let mut drive = Drive::new(self.direction, self.format, POISON[0]);
        drive_stream(&mut processor, &mut drive, signal, frame_positions)
            .map_err(|error| format!("{error}"))?;
        Ok(drive)
    }
}

// ------------------------------------------------------------------------- the driver ----

/// One run's accumulated evidence.
#[derive(Debug)]
struct Drive {
    direction: AudioDirection,
    format: StreamFormat,
    poison: i16,
    position: u64,
    samples: Vec<i16>,
    observations: Vec<DspObservation>,
    scratch_high_water: u32,
    length_violation: Option<ProcessError>,
}

impl Drive {
    fn new(direction: AudioDirection, format: StreamFormat, poison: i16) -> Self {
        Self {
            direction,
            format,
            poison,
            position: 0,
            samples: Vec::new(),
            observations: Vec::new(),
            scratch_high_water: 0,
            length_violation: None,
        }
    }
}

/// Offer one frame, with the workspace sized from the declaration and poisoned per §11.2.
fn feed<P: FrameProcessor>(
    processor: &mut P,
    drive: &mut Drive,
    chunk: &[i16],
    position: u64,
    discontinuity: Option<DiscontinuityKind>,
) -> Result<(), ProcessError> {
    let capability = processor.capability();
    let channels = usize::from(drive.format.channels()).max(1);
    let positions = chunk.len() / channels;
    let input_positions = u32::try_from(positions).unwrap_or(u32::MAX);
    let out_samples =
        usize_of(capability.max_output_positions(input_positions)).saturating_mul(channels);

    let mut output = vec![drive.poison; out_samples];
    let mut scratch_buffer = vec![drive.poison; usize_of(capability.scratch_samples())];
    let mut observations = Vec::with_capacity(usize_of(OBSERVATION_CAPACITY));

    let mut frame = DspFrame::new(drive.direction, drive.format, position, chunk);
    if let Some(kind) = discontinuity {
        frame = frame.with_discontinuity(kind);
    }

    let (result, written, produced) = {
        let mut scratch = Scratch::new(&mut scratch_buffer);
        let mut sink = FrameSink::new(&mut output, &mut observations, OBSERVATION_CAPACITY);
        let result = processor.process(&frame, &mut scratch, &mut sink);
        let evidence = (result, sink.written(), sink.samples().to_vec());
        drive.scratch_high_water = drive.scratch_high_water.max(scratch.requested());
        evidence
    };
    result?;

    let consumed = u64::try_from(positions).unwrap_or(u64::MAX);
    let produced_positions = u64::try_from(written / channels).unwrap_or(u64::MAX);
    let honoured = match capability.length() {
        LengthPolicy::Preserving => produced_positions == consumed,
        LengthPolicy::Bounded { max_positions } => produced_positions <= u64::from(max_positions),
    };
    if !honoured && drive.length_violation.is_none() {
        drive.length_violation = Some(ProcessError::LengthPolicyViolated {
            consumed,
            produced: produced_positions,
        });
    }

    drive.samples.extend_from_slice(&produced);
    drive.observations.extend(observations);
    drive.position = position.saturating_add(consumed);
    Ok(())
}

/// Offer a whole signal in frames of `frame_positions`, continuing the drive's epoch.
fn drive_stream<P: FrameProcessor>(
    processor: &mut P,
    drive: &mut Drive,
    signal: &[i16],
    frame_positions: usize,
) -> Result<(), ProcessError> {
    let channels = usize::from(drive.format.channels()).max(1);
    let step = frame_positions.max(1).saturating_mul(channels);
    let mut offset = 0;
    while offset < signal.len() {
        let end = offset.saturating_add(step).min(signal.len());
        let Some(chunk) = signal.get(offset..end) else {
            break;
        };
        let position = drive.position;
        feed(processor, drive, chunk, position, None)?;
        offset = end;
    }
    Ok(())
}

/// What offering a frame that should be refused actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Refused,
    Accepted,
    Wrote,
}

/// Offer a frame that must be refused, with its own throwaway workspace.
fn refuse<P: FrameProcessor>(
    processor: &mut P,
    capability: &DspCapability,
    frame: &DspFrame<'_>,
) -> Refusal {
    let channels = usize::from(frame.format().channels()).max(1);
    let positions = u32::try_from(frame.samples().len() / channels).unwrap_or(u32::MAX);
    let out_samples = usize_of(capability.max_output_positions(positions))
        .saturating_mul(channels)
        .max(1);
    let mut output = vec![0i16; out_samples];
    let mut scratch_buffer = vec![0i16; usize_of(capability.scratch_samples())];
    let mut observations = Vec::with_capacity(usize_of(OBSERVATION_CAPACITY));
    let mut scratch = Scratch::new(&mut scratch_buffer);
    let mut sink = FrameSink::new(&mut output, &mut observations, OBSERVATION_CAPACITY);
    match processor.process(frame, &mut scratch, &mut sink) {
        Ok(()) => Refusal::Accepted,
        Err(_) if sink.written() > 0 => Refusal::Wrote,
        Err(_) => Refusal::Refused,
    }
}

/// Drain a processor's tail into a buffer sized by its declared tail.
fn flush_all<P: FrameProcessor>(processor: &mut P, format: StreamFormat) -> Vec<i16> {
    let capability = processor.capability();
    let channels = usize::from(format.channels()).max(1);
    let mut output = vec![0i16; usize_of(capability.tail_positions()).saturating_mul(channels)];
    let mut observations = Vec::with_capacity(usize_of(OBSERVATION_CAPACITY));
    let mut sink = FrameSink::new(&mut output, &mut observations, OBSERVATION_CAPACITY);
    match processor.flush(&mut sink) {
        Ok(()) => sink.samples().to_vec(),
        Err(_) => Vec::new(),
    }
}

/// The frame size the harness will use: the reference size, or the processor's ceiling if smaller.
fn frame_positions(capability: &DspCapability, channels: usize) -> usize {
    let ceiling = usize_of(capability.max_frame_samples()) / channels.max(1);
    ceiling.clamp(1, REFERENCE_FRAME_POSITIONS)
}

/// A deterministic test signal that never reaches full scale, so §11's extremes stay `DSP-K12`'s.
fn signal(format: StreamFormat, positions: usize) -> Vec<i16> {
    let channels = usize::from(format.channels()).max(1);
    (0..positions.saturating_mul(channels))
        .map(|index| {
            let value = (i64::try_from(index).unwrap_or(0) * 2_617) % 19_997 - 9_998;
            i16::try_from(value).unwrap_or(0)
        })
        .collect()
}

fn wrong_kind(domain: ParameterDomain) -> ParameterValue {
    match domain {
        ParameterDomain::Flag => ParameterValue::Integer(0),
        ParameterDomain::Integer { .. } | ParameterDomain::Ratio { .. } => {
            ParameterValue::Flag(false)
        }
    }
}

fn out_of_range(domain: ParameterDomain) -> Option<ParameterValue> {
    match domain {
        ParameterDomain::Flag => None,
        ParameterDomain::Integer { min, max } => {
            if max < i64::MAX {
                Some(ParameterValue::Integer(max.saturating_add(1)))
            } else if min > i64::MIN {
                Some(ParameterValue::Integer(min.saturating_sub(1)))
            } else {
                None
            }
        }
        ParameterDomain::Ratio { min, max } => {
            if max < i32::MAX {
                Some(ParameterValue::Ratio(max.saturating_add(1)))
            } else if min > i32::MIN {
                Some(ParameterValue::Ratio(min.saturating_sub(1)))
            } else {
                None
            }
        }
    }
}

fn usize_of(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// A processor that cannot even run the harness's reference stream fails the check that needed it.
fn setup_failure(id: &'static str, name: &'static str, detail: &str) -> CheckOutcome {
    failed(
        id,
        name,
        format!("the processor could not run the harness's reference stream: {detail}"),
    )
}

/// `DSP-K1`: the declaration is internally admissible before anything is sized by it.
fn check_capability<P: FrameProcessor, F: FnMut() -> P>(factory: &mut F) -> CheckOutcome {
    let capability = factory().capability();
    match capability.validate() {
        Ok(()) => passed(
            CHECK_CAPABILITY,
            "capability",
            format!("`{}` declares an admissible shape", capability.id()),
        ),
        Err(error) => failed(CHECK_CAPABILITY, "capability", format!("{error}")),
    }
}

/// Run one check with its own panic guard: a processor that panics is a finding, and a harness that
/// dies with it produces no finding at all.
fn guarded(
    id: &'static str,
    name: &'static str,
    check: impl FnOnce() -> CheckOutcome,
) -> CheckOutcome {
    catch_unwind(AssertUnwindSafe(check)).unwrap_or_else(|_| {
        outcome(
            id,
            name,
            CheckStatus::Failed,
            "the processor panicked during this check".to_owned(),
        )
    })
}

fn outcome(
    id: &'static str,
    name: &'static str,
    status: CheckStatus,
    detail: String,
) -> CheckOutcome {
    CheckOutcome {
        id,
        name,
        status,
        detail,
    }
}

fn passed(id: &'static str, name: &'static str, detail: String) -> CheckOutcome {
    outcome(id, name, CheckStatus::Passed, detail)
}

fn failed(id: &'static str, name: &'static str, detail: String) -> CheckOutcome {
    outcome(id, name, CheckStatus::Failed, detail)
}

fn unproven(id: &'static str, name: &'static str, detail: String) -> CheckOutcome {
    outcome(id, name, CheckStatus::Unproven, detail)
}
