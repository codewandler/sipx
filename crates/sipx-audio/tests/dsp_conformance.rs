//! The custom call-DSP contract seen from outside the crate:
//! `docs/specs/custom-call-dsp.md` (`M-63`).
//!
//! Every processor in this file is built from `sipx_audio::dsp`'s **public** API and nothing else,
//! which is the point: the conformance harness has to accept a processor it has never seen, written
//! by somebody who cannot reach into the crate. An in-crate fixture would prove that the harness
//! runs, not that the contract is implementable from outside it.
//!
//! §12.1's three reference processors are here — `IDENT`, `DELAY1` and `GAIN2` — and so are the
//! eight deliberately broken ones §11.3 requires. A harness that has only ever seen a well-behaved
//! processor has proved nothing about its own ability to fail one, so each violating fixture breaks
//! exactly one invariant and this file asserts the harness names that invariant and no other.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_audio::analysis::{AudioDirection, DiscontinuityKind};
use sipx_audio::dsp::{
    CHECK_ALLOCATION, CHECK_CANCELLATION, CHECK_CAPABILITY, CHECK_CHUNK_BOUNDARY,
    CHECK_DETERMINISM, CHECK_DISCONTINUITY, CHECK_EXTREMES, CHECK_FRAME_REFUSAL, CHECK_LENGTH,
    CHECK_RESET, Conformance, DspCapability, DspFrame, DspObservation, DspResetCause,
    ExecutionPolicy, ExecutionProfile, FrameAdmission, FrameProcessor, FrameSink, LengthPolicy,
    Parameter, ParameterDomain, ParameterError, ParameterSpec, ParameterValue, ProcessError,
    ResetBehavior, Scratch, StreamFormat,
};

// ---------------------------------------------------------------- reference fixtures ----

/// §12.1 `IDENT`: the identity processor. Length preserving, stateless, no latency, no tail.
#[derive(Default)]
struct Ident {
    admission: FrameAdmission,
}

impl Ident {
    fn capability() -> DspCapability {
        DspCapability::new("ident")
            .with_channels(&[1, 2])
            .with_reset(ResetBehavior::Stateless)
    }
}

impl FrameProcessor for Ident {
    fn capability(&self) -> DspCapability {
        Self::capability()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        Self::capability().validate_parameters(parameters)
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), sipx_audio::dsp::FormatError> {
        self.admission
            .prepare(&Self::capability(), direction, format)
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        self.admission.admit(&Self::capability(), frame)?;
        sink.write(frame.samples())?;
        sink.emit(DspObservation::PassedThrough);
        Ok(())
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

/// §12.1 `DELAY1`: a one-position delay line. Length preserving, one position of latency and one
/// position of tail, and its state is cleared by every reset.
#[derive(Default)]
struct Delay1 {
    admission: FrameAdmission,
    held: i16,
    holding: bool,
}

impl Delay1 {
    fn capability() -> DspCapability {
        DspCapability::new("delay1")
            .with_latency_positions(1)
            .with_tail_positions(1)
            .with_reset(ResetBehavior::ClearsState)
    }
}

impl FrameProcessor for Delay1 {
    fn capability(&self) -> DspCapability {
        Self::capability()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        Self::capability().validate_parameters(parameters)
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), sipx_audio::dsp::FormatError> {
        self.admission
            .prepare(&Self::capability(), direction, format)?;
        self.held = 0;
        self.holding = false;
        Ok(())
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        let admitted = self.admission.admit(&Self::capability(), frame)?;
        if admitted.discontinuity().is_some() {
            self.held = 0;
            self.holding = false;
        }
        for sample in frame.samples() {
            sink.push(self.held)?;
            self.held = *sample;
            self.holding = true;
        }
        Ok(())
    }

    fn flush(&mut self, sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.admission.admit_flush()?;
        sink.push(self.held)?;
        self.held = 0;
        self.holding = false;
        Ok(())
    }

    fn reset(&mut self, _cause: DspResetCause) {
        self.admission.reset();
        self.held = 0;
        self.holding = false;
    }

    fn cancel(&mut self) {
        self.admission.cancel();
        self.held = 0;
        self.holding = false;
    }

    fn retained(&self) -> u32 {
        u32::from(self.holding)
    }
}

/// §12.1 `GAIN2`: a saturating power-of-two gain with one closed integer parameter.
#[derive(Default)]
struct Gain2 {
    admission: FrameAdmission,
    shift: u32,
}

impl Gain2 {
    const SHIFT: &'static [ParameterSpec] = &[ParameterSpec::new(
        "shift",
        ParameterDomain::Integer { min: 0, max: 2 },
    )];

    fn capability() -> DspCapability {
        DspCapability::new("gain2")
            .with_parameters(Self::SHIFT)
            .with_reset(ResetBehavior::Stateless)
    }
}

impl FrameProcessor for Gain2 {
    fn capability(&self) -> DspCapability {
        Self::capability()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        let capability = Self::capability();
        capability.validate_parameters(parameters)?;
        // Validated completely before anything is assigned: a refused set leaves the previous
        // parameter state in force, untouched (§8.4).
        for parameter in parameters {
            if let ParameterValue::Integer(shift) = parameter.value() {
                self.shift = u32::try_from(shift).unwrap_or(0);
            }
        }
        Ok(())
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), sipx_audio::dsp::FormatError> {
        self.admission
            .prepare(&Self::capability(), direction, format)
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        self.admission.admit(&Self::capability(), frame)?;
        let mut saturated = 0u32;
        for sample in frame.samples() {
            let scaled = i32::from(*sample) * (1 << self.shift);
            let clamped = scaled.clamp(i32::from(i16::MIN), i32::from(i16::MAX));
            if clamped != scaled {
                saturated += 1;
            }
            sink.push(i16::try_from(clamped).unwrap_or(0))?;
        }
        if saturated > 0 {
            sink.emit(DspObservation::Saturated {
                positions: saturated,
            });
        }
        Ok(())
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

// ---------------------------------------------------------------- violating fixtures ----

/// §11.3: a `DELAY1` that ignores `reset`. Its delay line survives an epoch that no longer exists.
#[derive(Default)]
struct LeakyReset {
    inner: Delay1,
}

impl FrameProcessor for LeakyReset {
    fn capability(&self) -> DspCapability {
        Delay1::capability().with_id("leaky-reset")
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        self.inner.configure(parameters)
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), sipx_audio::dsp::FormatError> {
        self.inner.prepare(direction, format)
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        // The admission machinery still runs, so the epoch is tracked correctly; what leaks is the
        // one piece of sample memory this processor owns.
        let admitted = self.inner.admission.admit(&self.capability(), frame)?;
        let _ = admitted;
        let _ = scratch;
        for sample in frame.samples() {
            sink.push(self.inner.held)?;
            self.inner.held = *sample;
            self.inner.holding = true;
        }
        Ok(())
    }

    fn flush(&mut self, sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.inner.flush(sink)
    }

    fn reset(&mut self, _cause: DspResetCause) {
        // The violation: the admission state restarts, the retained count is dutifully released,
        // and the one sample of audio the processor is actually holding survives the epoch it
        // belonged to.
        self.inner.admission.reset();
        self.inner.holding = false;
    }

    fn cancel(&mut self) {
        self.inner.cancel();
    }

    fn retained(&self) -> u32 {
        self.inner.retained()
    }
}

/// §11.3: a processor that keeps working after `cancel`.
#[derive(Default)]
struct ZombieCancel {
    inner: Delay1,
}

impl FrameProcessor for ZombieCancel {
    fn capability(&self) -> DspCapability {
        Delay1::capability().with_id("zombie-cancel")
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        self.inner.configure(parameters)
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), sipx_audio::dsp::FormatError> {
        self.inner.prepare(direction, format)
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        self.inner.process(frame, scratch, sink)
    }

    fn flush(&mut self, sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.inner.flush(sink)
    }

    fn reset(&mut self, cause: DspResetCause) {
        self.inner.reset(cause);
    }

    fn cancel(&mut self) {
        // The violation: cancellation is ignored, so the processor stays live and keeps a sample.
    }

    fn retained(&self) -> u32 {
        self.inner.retained()
    }
}

/// §11.3: a processor whose output depends on where the caller cut the stream into frames.
#[derive(Default)]
struct ChunkDependent {
    admission: FrameAdmission,
}

impl FrameProcessor for ChunkDependent {
    fn capability(&self) -> DspCapability {
        DspCapability::new("chunk-dependent")
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        self.capability().validate_parameters(parameters)
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), sipx_audio::dsp::FormatError> {
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
        // The violation: the first position of every *frame* is zeroed, so the same stream cut
        // differently is a different signal.
        for (index, sample) in frame.samples().iter().enumerate() {
            sink.push(if index == 0 { 0 } else { *sample })?;
        }
        Ok(())
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

/// §11.3: a processor that asks for more scratch than it declared, and hides the refusal.
#[derive(Default)]
struct ScratchHog {
    admission: FrameAdmission,
}

impl FrameProcessor for ScratchHog {
    fn capability(&self) -> DspCapability {
        DspCapability::new("scratch-hog").with_scratch_samples(4)
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        self.capability().validate_parameters(parameters)
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), sipx_audio::dsp::FormatError> {
        self.admission
            .prepare(&self.capability(), direction, format)
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        self.admission.admit(&self.capability(), frame)?;
        // The violation: eight positions of scratch against a declaration of four. Swallowing the
        // refusal is what makes this worth a check — the workspace bound holds either way, and the
        // declaration is what turns out to be false.
        let _ = scratch.take(8);
        sink.write(frame.samples())?;
        Ok(())
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

/// §11.3: a processor that declares `Preserving` and writes one position for every two it
/// consumed. The parity lives in its state rather than in the frame, so the lie is exactly the
/// length policy and not also chunk dependence.
#[derive(Default)]
struct LengthLiar {
    admission: FrameAdmission,
    parity: bool,
}

impl FrameProcessor for LengthLiar {
    fn capability(&self) -> DspCapability {
        DspCapability::new("length-liar")
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        self.capability().validate_parameters(parameters)
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), sipx_audio::dsp::FormatError> {
        self.admission
            .prepare(&self.capability(), direction, format)?;
        self.parity = false;
        Ok(())
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        self.admission.admit(&self.capability(), frame)?;
        for sample in frame.samples() {
            // The violation: half the positions in, all of them declared preserved.
            if !self.parity {
                sink.push(*sample)?;
            }
            self.parity = !self.parity;
        }
        Ok(())
    }

    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.admission.admit_flush()
    }

    fn reset(&mut self, _cause: DspResetCause) {
        self.admission.reset();
        self.parity = false;
    }

    fn cancel(&mut self) {
        self.admission.cancel();
        self.parity = false;
    }

    fn retained(&self) -> u32 {
        0
    }
}

/// §11.3: a processor that consumes a frame into its state and only then refuses it. On every
/// accepted frame it is `DELAY1` exactly; the difference is only ever visible after a refusal.
#[derive(Default)]
struct SloppyRefusal {
    admission: FrameAdmission,
    held: i16,
    holding: bool,
}

impl FrameProcessor for SloppyRefusal {
    fn capability(&self) -> DspCapability {
        Delay1::capability().with_id("sloppy-refusal")
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        self.capability().validate_parameters(parameters)
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), sipx_audio::dsp::FormatError> {
        self.admission
            .prepare(&self.capability(), direction, format)?;
        self.held = 0;
        self.holding = false;
        Ok(())
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        // The violation: the delay line advances before the frame is admitted, so a *refused*
        // frame still changes what the next accepted one produces.
        let entering = self.held;
        if let Some(last) = frame.samples().last() {
            self.held = *last;
        }
        let admitted = self.admission.admit(&self.capability(), frame)?;
        let mut held = if admitted.discontinuity().is_some() {
            0
        } else {
            entering
        };
        for sample in frame.samples() {
            sink.push(held)?;
            held = *sample;
            self.holding = true;
        }
        self.held = held;
        Ok(())
    }

    fn flush(&mut self, sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.admission.admit_flush()?;
        sink.push(self.held)?;
        self.held = 0;
        self.holding = false;
        Ok(())
    }

    fn reset(&mut self, _cause: DspResetCause) {
        self.admission.reset();
        self.held = 0;
        self.holding = false;
    }

    fn cancel(&mut self) {
        self.admission.cancel();
        self.held = 0;
        self.holding = false;
    }

    fn retained(&self) -> u32 {
        u32::from(self.holding)
    }
}

/// §11.3: a processor that panics on a sample value a real call can carry.
#[derive(Default)]
struct Panicky {
    admission: FrameAdmission,
}

impl FrameProcessor for Panicky {
    fn capability(&self) -> DspCapability {
        DspCapability::new("panicky")
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        self.capability().validate_parameters(parameters)
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), sipx_audio::dsp::FormatError> {
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
        for sample in frame.samples() {
            // The violation: `-i16::MIN` is the classic one, written out so it is a panic and not
            // a silent wrap in release.
            assert!(*sample != i16::MIN, "this processor cannot negate i16::MIN");
            sink.push(-*sample)?;
        }
        Ok(())
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

/// §11.3: a processor that reads scratch it never wrote, and so is a function of the caller's
/// leftovers rather than of its input.
#[derive(Default)]
struct ScratchReader {
    admission: FrameAdmission,
}

impl FrameProcessor for ScratchReader {
    fn capability(&self) -> DspCapability {
        DspCapability::new("scratch-reader").with_scratch_samples(8)
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        self.capability().validate_parameters(parameters)
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), sipx_audio::dsp::FormatError> {
        self.admission
            .prepare(&self.capability(), direction, format)
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        self.admission.admit(&self.capability(), frame)?;
        let leftovers = scratch.take(1)?;
        // The violation: scratch contents on entry are unspecified, so this reads whatever the
        // caller last left there.
        let bias = leftovers[0];
        for sample in frame.samples() {
            sink.push(sample.saturating_add(bias))?;
        }
        Ok(())
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

// ---------------------------------------------------------------- the well-behaved runs ----

fn mono8k() -> StreamFormat {
    StreamFormat::new(8_000, 1).unwrap()
}

/// A reference run of `IDENT` passes every check the harness has.
#[test]
fn the_identity_processor_conforms() {
    let report = Conformance::new().run(Ident::default);
    assert!(report.passed(), "{report}");
}

/// A reference run of `DELAY1` passes every check, latency and tail included.
#[test]
fn the_one_position_delay_conforms() {
    let report = Conformance::new().run(Delay1::default);
    assert!(report.passed(), "{report}");
}

/// A reference run of `GAIN2` passes every check, its closed parameter included.
#[test]
fn the_saturating_gain_conforms() {
    let report = Conformance::new().run(Gain2::default);
    assert!(report.passed(), "{report}");
}

/// §11.2: a stereo stream is the same contract, and a processor that declares two channels runs
/// under it unchanged.
#[test]
fn a_declared_stereo_processor_conforms() {
    let report = Conformance::new()
        .with_format(StreamFormat::new(16_000, 2).unwrap())
        .with_direction(AudioDirection::Outbound)
        .run(Ident::default);
    assert!(report.passed(), "{report}");
}

// ---------------------------------------------------------------- the caught violations ----

/// The harness reports exactly the invariant a fixture breaks, so the report is a diagnosis.
fn assert_caught(report: &sipx_audio::dsp::ConformanceReport, expected: &str) {
    assert!(!report.passed(), "{report}");
    assert!(
        report.failed(expected),
        "expected {expected} to fail; got {report}"
    );
}

/// §11.3: a processor that ignores `reset` is caught by the reset check.
#[test]
fn the_harness_catches_a_processor_that_ignores_reset() {
    let report = Conformance::new().run(LeakyReset::default);
    assert_caught(&report, CHECK_RESET);
}

/// §11.3: a processor that keeps working after `cancel` is caught by the cancellation check.
#[test]
fn the_harness_catches_a_processor_that_ignores_cancellation() {
    let report = Conformance::new().run(ZombieCancel::default);
    assert_caught(&report, CHECK_CANCELLATION);
}

/// §11.3: a processor whose output depends on the caller's frame sizes is caught by the
/// chunk-boundary check.
#[test]
fn the_harness_catches_a_chunk_dependent_processor() {
    let report = Conformance::new().run(ChunkDependent::default);
    assert_caught(&report, CHECK_CHUNK_BOUNDARY);
}

/// §11.3: a processor that asks for more scratch than it declared is caught by the allocation
/// check, even though the workspace bound itself held.
#[test]
fn the_harness_catches_a_processor_that_exceeds_its_declared_scratch() {
    let report = Conformance::new().run(ScratchHog::default);
    assert_caught(&report, CHECK_ALLOCATION);
}

/// §11.3: a `Preserving` processor that produces fewer positions than it consumed is caught by the
/// length check.
#[test]
fn the_harness_catches_a_processor_that_breaks_its_length_policy() {
    let report = Conformance::new().run(LengthLiar::default);
    assert_caught(&report, CHECK_LENGTH);
}

/// §11.3: a processor that mutates state before refusing a frame is caught by the frame-refusal
/// check, which is exactly the "no partial state mutation" obligation.
#[test]
fn the_harness_catches_a_refusal_that_mutated_state() {
    let report = Conformance::new().run(SloppyRefusal::default);
    assert_caught(&report, CHECK_FRAME_REFUSAL);
}

/// §11.3: a processor that panics on a sample a real call can carry is caught rather than taking
/// the harness down with it.
#[test]
fn the_harness_catches_a_panicking_processor() {
    let report = Conformance::new().run(Panicky::default);
    assert_caught(&report, CHECK_EXTREMES);
}

/// §11.3: a processor that reads unwritten scratch is caught by the determinism check, because the
/// harness poisons the workspace differently on each run.
#[test]
fn the_harness_catches_a_processor_that_reads_unwritten_scratch() {
    let report = Conformance::new().run(ScratchReader::default);
    assert_caught(&report, CHECK_DETERMINISM);
}

/// §11.1: every check the harness declares runs, and none of them is silently absent from a
/// report. An omitted check is how a harness starts proving less than it says it does.
#[test]
fn every_declared_check_appears_in_every_report() {
    let report = Conformance::new().run(Ident::default);
    for id in sipx_audio::dsp::CHECKS {
        assert!(
            report.checks().iter().any(|check| check.id() == *id),
            "{id} is declared but absent from the report: {report}"
        );
    }
    assert_eq!(report.checks().len(), sipx_audio::dsp::CHECKS.len());
}

/// §11.1: a check the harness cannot prove is reported as unproven, never as a pass. Heap growth
/// inside a processor is the standing example: this workspace forbids `unsafe`, so no counting
/// allocator can be installed and the declaration is held only where it can be measured.
#[test]
fn an_unprovable_check_is_reported_as_unproven_rather_than_passed() {
    let report = Conformance::new().run(Ident::default);
    let allocation = report
        .checks()
        .iter()
        .find(|check| check.id() == CHECK_ALLOCATION)
        .unwrap();
    assert!(
        !allocation.detail().is_empty(),
        "every outcome states what it measured"
    );
    assert!(
        report.unproven().count() > 0,
        "the heap component of the allocation bound is not observable here and must say so: \
         {report}"
    );
}

// ---------------------------------------------------------------- §12 sample vectors ----

/// Run one frame through a processor and return what it wrote.
fn run_frame(processor: &mut dyn FrameProcessor, frame: &DspFrame<'_>) -> Vec<i16> {
    let capability = processor.capability();
    let positions = u32::try_from(frame.positions()).unwrap();
    let mut output = vec![
        0i16;
        (capability.max_output_positions(positions) * u32::from(frame.format().channels()))
            as usize
    ];
    let mut scratch_buffer = vec![0i16; capability.scratch_samples() as usize];
    let mut observations = Vec::with_capacity(8);
    let mut scratch = Scratch::new(&mut scratch_buffer);
    let mut sink = FrameSink::new(&mut output, &mut observations, 8);
    processor.process(frame, &mut scratch, &mut sink).unwrap();
    sink.samples().to_vec()
}

/// DSP-V1: the identity processor returns its input unchanged, extremes included.
#[test]
fn dsp_v1_identity_returns_its_input() {
    let mut processor = Ident::default();
    processor
        .prepare(AudioDirection::Inbound, mono8k())
        .unwrap();
    let samples = [1i16, -1, i16::MAX, i16::MIN];
    let frame = DspFrame::new(AudioDirection::Inbound, mono8k(), 0, &samples);
    assert_eq!(run_frame(&mut processor, &frame), samples);
}

/// DSP-V2: a one-position delay line delays across a frame boundary and flushes its tail.
#[test]
fn dsp_v2_the_delay_line_crosses_frame_boundaries() {
    let mut processor = Delay1::default();
    processor
        .prepare(AudioDirection::Inbound, mono8k())
        .unwrap();

    let first = [1i16, 2, 3];
    let frame = DspFrame::new(AudioDirection::Inbound, mono8k(), 0, &first);
    assert_eq!(run_frame(&mut processor, &frame), vec![0, 1, 2]);

    let second = [4i16, 5, 6];
    let frame = DspFrame::new(AudioDirection::Inbound, mono8k(), 3, &second);
    assert_eq!(run_frame(&mut processor, &frame), vec![3, 4, 5]);

    assert_eq!(processor.retained(), 1);
    let mut output = [0i16; 1];
    let mut observations = Vec::with_capacity(2);
    let mut sink = FrameSink::new(&mut output, &mut observations, 2);
    processor.flush(&mut sink).unwrap();
    assert_eq!(sink.samples(), [6]);
}

/// DSP-V3: a reset discards the retained position rather than flushing it, and the next epoch
/// starts at position 0.
#[test]
fn dsp_v3_a_reset_discards_the_retained_position() {
    let mut processor = Delay1::default();
    processor
        .prepare(AudioDirection::Inbound, mono8k())
        .unwrap();
    let first = [1i16, 2, 3];
    let _ = run_frame(
        &mut processor,
        &DspFrame::new(AudioDirection::Inbound, mono8k(), 0, &first),
    );

    processor.reset(DspResetCause::Requested);
    assert_eq!(processor.retained(), 0);

    let second = [4i16, 5, 6];
    let frame = DspFrame::new(AudioDirection::Inbound, mono8k(), 0, &second);
    assert_eq!(run_frame(&mut processor, &frame), vec![0, 4, 5]);
}

/// DSP-V4: a `Realign` opens a new epoch at position 0 and its reset runs before the flagged
/// frame's own samples, so the frame produces exactly what a fresh processor would.
#[test]
fn dsp_v4_a_realign_restarts_the_epoch_before_its_own_samples() {
    let mut processor = Delay1::default();
    processor
        .prepare(AudioDirection::Inbound, mono8k())
        .unwrap();
    let first = [1i16, 2, 3];
    let _ = run_frame(
        &mut processor,
        &DspFrame::new(AudioDirection::Inbound, mono8k(), 0, &first),
    );

    let second = [4i16, 5, 6];
    let flagged = DspFrame::new(AudioDirection::Inbound, mono8k(), 0, &second)
        .with_discontinuity(DiscontinuityKind::Realign);
    assert_eq!(run_frame(&mut processor, &flagged), vec![0, 4, 5]);
}

/// DSP-V5: the position rules of §3.5, one refusal at a time, each leaving the stream where it
/// stood.
#[test]
fn dsp_v5_position_rules_refuse_without_changing_state() {
    let mut processor = Delay1::default();
    processor
        .prepare(AudioDirection::Inbound, mono8k())
        .unwrap();
    let samples = [1i16, 2, 3];

    // The first frame of an epoch must open it at position 0.
    let mut output = [0i16; 3];
    let mut observations = Vec::with_capacity(2);
    let mut scratch_buffer = [0i16; 0];
    let mut scratch = Scratch::new(&mut scratch_buffer);
    let mut sink = FrameSink::new(&mut output, &mut observations, 2);
    let late = DspFrame::new(AudioDirection::Inbound, mono8k(), 7, &samples);
    assert_eq!(
        processor.process(&late, &mut scratch, &mut sink),
        Err(ProcessError::PositionNotContiguous)
    );

    // The refusal changed nothing: position 0 is still what the epoch expects.
    let frame = DspFrame::new(AudioDirection::Inbound, mono8k(), 0, &samples);
    assert_eq!(run_frame(&mut processor, &frame), vec![0, 1, 2]);

    // An unflagged gap is a broken caller, not a loss to smooth over.
    let mut scratch = Scratch::new(&mut scratch_buffer);
    let mut sink = FrameSink::new(&mut output, &mut observations, 2);
    let gap = DspFrame::new(AudioDirection::Inbound, mono8k(), 9, &samples);
    assert_eq!(
        processor.process(&gap, &mut scratch, &mut sink),
        Err(ProcessError::PositionNotContiguous)
    );

    // The same gap, flagged, is accepted: the seam says what happened.
    let flagged = DspFrame::new(AudioDirection::Inbound, mono8k(), 9, &samples)
        .with_discontinuity(DiscontinuityKind::Loss);
    assert_eq!(run_frame(&mut processor, &flagged), vec![0, 1, 2]);
}

/// DSP-V6: the frame refusals of §8.3, each typed and each mutating nothing.
#[test]
fn dsp_v6_malformed_frames_are_refused_by_type() {
    let mut processor = Ident::default();
    processor
        .prepare(AudioDirection::Inbound, mono8k())
        .unwrap();

    let mut output = [0i16; 8];
    let mut observations = Vec::with_capacity(2);
    let mut scratch_buffer = [0i16; 0];

    let empty: [i16; 0] = [];
    let cases: [(DspFrame<'_>, ProcessError); 3] = [
        (
            DspFrame::new(AudioDirection::Inbound, mono8k(), 0, &empty),
            ProcessError::MalformedFrame,
        ),
        (
            DspFrame::new(AudioDirection::Outbound, mono8k(), 0, &[1i16, 2]),
            ProcessError::DirectionMismatch,
        ),
        (
            DspFrame::new(
                AudioDirection::Inbound,
                StreamFormat::new(16_000, 1).unwrap(),
                0,
                &[1i16, 2],
            ),
            ProcessError::FormatMismatch,
        ),
    ];
    for (frame, expected) in cases {
        let mut scratch = Scratch::new(&mut scratch_buffer);
        let mut sink = FrameSink::new(&mut output, &mut observations, 2);
        assert_eq!(
            processor.process(&frame, &mut scratch, &mut sink),
            Err(expected)
        );
        assert_eq!(sink.written(), 0, "a refusal writes nothing");
    }

    // And the stream still stands where it did.
    let frame = DspFrame::new(AudioDirection::Inbound, mono8k(), 0, &[7i16, 8]);
    assert_eq!(run_frame(&mut processor, &frame), vec![7, 8]);
}

/// DSP-V7: the workspace refuses to grow — the sink past its capacity and scratch past the
/// declaration the caller sized it from.
#[test]
fn dsp_v7_the_workspace_refuses_to_grow() {
    let mut output = [0i16; 2];
    let mut observations = Vec::with_capacity(2);
    let mut sink = FrameSink::new(&mut output, &mut observations, 2);
    assert_eq!(sink.write(&[1i16, 2]), Ok(()));
    assert_eq!(sink.push(3), Err(ProcessError::OutputOverflow));
    assert_eq!(sink.written(), 2, "a refused write leaves the sink alone");

    let mut scratch_buffer = [0i16; 4];
    let mut scratch = Scratch::new(&mut scratch_buffer);
    assert!(scratch.take(4).is_ok());
    assert_eq!(
        scratch.take(5).err(),
        Some(ProcessError::ScratchExhausted {
            requested: 5,
            available: 4
        })
    );
    assert_eq!(
        scratch.requested(),
        5,
        "a refused request is still a request, and the declaration is what it is held to"
    );
}

/// DSP-V8: the observation queue is bounded and loses deterministically, exactly as the analysis
/// contract's does.
#[test]
fn dsp_v8_the_observation_queue_coalesces_rather_than_growing() {
    let mut output = [0i16; 4];
    let mut observations = Vec::with_capacity(2);
    let mut sink = FrameSink::new(&mut output, &mut observations, 2);
    sink.emit(DspObservation::PassedThrough);
    sink.emit(DspObservation::PassedThrough);
    sink.emit(DspObservation::PassedThrough);
    sink.emit(DspObservation::PassedThrough);
    assert_eq!(
        observations,
        vec![
            DspObservation::PassedThrough,
            DspObservation::Lost { count: 3 }
        ]
    );
}

/// DSP-V9: parameters are a closed, finite vocabulary, and a refused set leaves the previous one
/// in force.
#[test]
fn dsp_v9_parameters_are_closed_and_refusals_are_total() {
    let mut processor = Gain2::default();
    processor
        .prepare(AudioDirection::Inbound, mono8k())
        .unwrap();
    processor
        .configure(&[Parameter::new("shift", ParameterValue::Integer(1))])
        .unwrap();

    let frame = DspFrame::new(AudioDirection::Inbound, mono8k(), 0, &[1i16, 2, i16::MAX]);
    assert_eq!(run_frame(&mut processor, &frame), vec![2, 4, i16::MAX]);

    assert_eq!(
        processor.configure(&[Parameter::new("shift", ParameterValue::Integer(9))]),
        Err(ParameterError::OutOfRange { id: "shift" })
    );
    assert_eq!(
        processor.configure(&[Parameter::new("gain", ParameterValue::Integer(1))]),
        Err(ParameterError::Unknown { id: "gain" })
    );
    assert_eq!(
        processor.configure(&[Parameter::new("shift", ParameterValue::Flag(true))]),
        Err(ParameterError::KindMismatch { id: "shift" })
    );

    // Every refusal above left `shift = 1` in force.
    let frame = DspFrame::new(AudioDirection::Inbound, mono8k(), 3, &[1i16, 2, i16::MAX]);
    assert_eq!(run_frame(&mut processor, &frame), vec![2, 4, i16::MAX]);
}

/// DSP-V10: the three execution profiles, and exactly which of them may claim that over-budget
/// work cannot stall RTP.
#[test]
fn dsp_v10_only_two_profiles_contain_an_overrun() {
    assert!(ExecutionProfile::ProvenInline.contains_overrun());
    assert!(ExecutionProfile::SupervisedIsolated.contains_overrun());
    assert!(!ExecutionProfile::TrustedCooperativeNative.contains_overrun());

    // The two containment mechanisms are different, and the contract says which is which: the
    // inline profile bounds the work, the isolated profile bounds the wait.
    assert_eq!(
        ExecutionProfile::ProvenInline.containment(),
        sipx_audio::dsp::OverrunContainment::BoundedWork
    );
    assert_eq!(
        ExecutionProfile::SupervisedIsolated.containment(),
        sipx_audio::dsp::OverrunContainment::BoundedWait
    );
    assert_eq!(
        ExecutionProfile::TrustedCooperativeNative.containment(),
        sipx_audio::dsp::OverrunContainment::None
    );

    // Only the isolated profile can abandon a frame that missed its deadline; a synchronous call
    // on the media worker can only be measured once it has returned.
    assert_eq!(
        ExecutionProfile::SupervisedIsolated.deadline_action(),
        sipx_audio::dsp::DeadlineAction::AbandonResult
    );
    assert_eq!(
        ExecutionProfile::ProvenInline.deadline_action(),
        sipx_audio::dsp::DeadlineAction::AccountAfterReturn
    );
    assert_eq!(
        ExecutionProfile::TrustedCooperativeNative.deadline_action(),
        sipx_audio::dsp::DeadlineAction::AccountAfterReturn
    );

    // Two of the three run application code on the media worker; only one of those two is
    // permitted to call itself contained.
    assert!(ExecutionProfile::TrustedCooperativeNative.runs_on_media_worker());
    assert!(!ExecutionProfile::SupervisedIsolated.runs_on_media_worker());
}

/// DSP-V11: a capability that describes something the contract cannot admit is refused before any
/// caller sizes a buffer from it.
#[test]
fn dsp_v11_a_malformed_capability_is_refused() {
    const REPEATED: &[ParameterSpec] = &[
        ParameterSpec::new("a", ParameterDomain::Flag),
        ParameterSpec::new("a", ParameterDomain::Flag),
    ];
    assert!(DspCapability::new("ok").validate().is_ok());
    assert!(
        DspCapability::new("no-channels")
            .with_channels(&[])
            .validate()
            .is_err()
    );
    assert!(
        DspCapability::new("too-many-channels")
            .with_channels(&[9])
            .validate()
            .is_err()
    );
    assert!(
        DspCapability::new("huge-frame")
            .with_max_frame_samples(70_000)
            .validate()
            .is_err()
    );
    assert!(
        DspCapability::new("duplicate")
            .with_parameters(REPEATED)
            .validate()
            .is_err()
    );
    assert!(
        DspCapability::new("empty-deadline")
            .with_execution(
                ExecutionPolicy::new(ExecutionProfile::SupervisedIsolated).with_deadline_frames(0)
            )
            .validate()
            .is_err()
    );
}

/// DSP-V12: a `Bounded` length policy sizes the caller's sink, and a `Preserving` one is exactly
/// its input.
#[test]
fn dsp_v12_the_length_policy_sizes_the_sink() {
    let preserving = DspCapability::new("p");
    assert_eq!(preserving.max_output_positions(160), 160);

    let bounded = DspCapability::new("b").with_length(LengthPolicy::Bounded { max_positions: 4 });
    assert_eq!(bounded.max_output_positions(160), 4);
}

/// §11: the check identifiers are stable strings, because a downstream story that asserts on one
/// is asserting on a name.
#[test]
fn the_check_identifiers_are_the_ones_the_spec_names() {
    assert_eq!(CHECK_CAPABILITY, "DSP-K1");
    assert_eq!(CHECK_DISCONTINUITY, "DSP-K10");
    assert_eq!(CHECK_LENGTH, "DSP-K11");
    assert_eq!(CHECK_EXTREMES, "DSP-K12");
}
