//! Interchangeable noise reduction seen from outside the crate:
//! `docs/specs/call-dsp-noise-reduction.md` (`M-66`).
//!
//! Everything here is reached through `sipx_audio::dsp`'s **public** API and nothing else. That is
//! not a style preference: §4 of the spec makes "a second implementation can be substituted without
//! the call layer knowing" the deliverable, and an interface exercised through a crate-private
//! handle would prove the opposite of what this file exists to prove.
//!
//! So this file carries a **second reducer** — [`FixedFloorSubtractor`] — written against the
//! public trait from outside `sipx-audio`, with different arithmetic, a different rate list, a
//! different channel list, no warm-up, no adaptive state at all, a different reset behaviour and a
//! different execution profile. Every lifecycle and conformance assertion below runs against both,
//! and §4.4's `dyn NoiseReducer` vector drives them through code that cannot name either.
//!
//! The sample-exact vectors run where the arithmetic is checkable on paper. NR-V3 through NR-V7
//! drive full-scale alternation at 8,000 Hz — the folding frequency, where both one-pole sections
//! are exactly zero, so the whole signal lands in the high band and the settled output is the input
//! times one declared gain and nothing else.
//!
//! NR-V18 through NR-V22 are `M-114`'s: §10's producer for the hint §5.5 consumes. They live here
//! rather than in a file of their own because the measurement they exist for is §8's corpus
//! measured twice, and two definitions of that corpus could drift apart while both stayed green.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_audio::analysis::{
    AnalysisFrame, AnalysisProfile, AudioAnalyzer, AudioDirection, DiscontinuityKind, Observation,
    ResetCause, VoiceEndCause,
};
use sipx_audio::dsp::noise::{
    ActivityHint, ActivityInput, HintCause, HintPolicy, HostRequirement, MAX_WARM_UP_POSITIONS,
    NOISE_REDUCTION_IDS, NoiseReducer, NoiseReduction, NoiseReductionError, SUBBAND_SUPPRESSOR,
    SubbandSuppressor,
};
use sipx_audio::dsp::{
    CHECK_ALLOCATION, CapabilityError, Conformance, DspCapability, DspFrame, DspObservation,
    DspResetCause, ExecutionPolicy, ExecutionProfile, FormatError, FrameAdmission, FrameProcessor,
    FrameSink, LengthPolicy, Parameter, ParameterDomain, ParameterError, ParameterSpec,
    ParameterValue, ProcessError, RateSupport, ResetBehavior, Scratch, StreamFormat,
};

// ------------------------------------------------------------------------- the driver ----

/// One replay's accumulated evidence: what the reducer wrote and what it said about it.
#[derive(Debug, Default)]
struct Run {
    samples: Vec<i16>,
    observations: Vec<DspObservation>,
    position: u64,
}

impl Run {
    /// Offer one frame, continuing this replay's epoch.
    fn feed(
        &mut self,
        reducer: &mut dyn NoiseReducer,
        format: StreamFormat,
        input: &[i16],
    ) -> Result<(), ProcessError> {
        self.feed_at(reducer, format, input, self.position, None)
    }

    /// Offer one frame at a stated position, optionally carrying a declared break.
    fn feed_at(
        &mut self,
        reducer: &mut dyn NoiseReducer,
        format: StreamFormat,
        input: &[i16],
        position: u64,
        discontinuity: Option<DiscontinuityKind>,
    ) -> Result<(), ProcessError> {
        let capability = reducer.capability();
        let positions = u32::try_from(input.len()).unwrap() / u32::from(format.channels());
        let room = capability.max_output_positions(positions) * u32::from(format.channels());
        let mut output = vec![0i16; usize::try_from(room).unwrap()];
        let mut observations = Vec::new();
        let mut scratch = vec![0i16; usize::try_from(capability.scratch_samples()).unwrap()];

        let mut frame = DspFrame::new(AudioDirection::Inbound, format, position, input);
        if let Some(kind) = discontinuity {
            frame = frame.with_discontinuity(kind);
        }
        let outcome = {
            let mut scratch = Scratch::new(&mut scratch);
            let mut sink = FrameSink::new(&mut output, &mut observations, 32);
            let outcome = reducer.process(&frame, &mut scratch, &mut sink);
            if outcome.is_ok() {
                self.samples.extend_from_slice(sink.samples());
            }
            outcome
        };
        if outcome.is_ok() {
            self.observations.append(&mut observations);
            self.position = position + u64::from(positions);
        }
        outcome
    }

    /// The tail of this replay, from `from` to the end.
    fn tail(&self, from: usize) -> &[i16] {
        &self.samples[from..]
    }
}

/// Prepare a reducer on `D8` — inbound, 8,000 Hz, mono — and hand back the format.
fn d8(reducer: &mut dyn NoiseReducer) -> StreamFormat {
    let format = StreamFormat::default();
    reducer
        .prepare(AudioDirection::Inbound, format)
        .expect("D8 is inside every reducer's declaration");
    format
}

/// Drive one whole stream through a reducer in frames of `chunk` positions.
fn replay(reducer: &mut dyn NoiseReducer, input: &[i16], chunk: usize) -> Run {
    let format = d8(reducer);
    let mut run = Run::default();
    for window in input.chunks(chunk) {
        run.feed(reducer, format, window).expect("a valid frame");
    }
    run
}

fn ratio(id: &'static str, thousandths: i32) -> Parameter {
    Parameter::new(id, ParameterValue::Ratio(thousandths))
}

fn integer(id: &'static str, value: i64) -> Parameter {
    Parameter::new(id, ParameterValue::Integer(value))
}

fn flag(id: &'static str, value: bool) -> Parameter {
    Parameter::new(id, ParameterValue::Flag(value))
}

/// Full-scale alternation: at 8,000 Hz this is the folding frequency, where both of the baseline's
/// sections are exactly zero and the whole signal is in the high band (§9, NR-V3).
fn alternating(positions: usize, amplitude: i16) -> Vec<i16> {
    (0..positions)
        .map(|n| if n % 2 == 0 { amplitude } else { -amplitude })
        .collect()
}

// --------------------------------------------------------- the external fixture (§4.4) ----

/// The `floor_magnitude` this fixture subtracts, in sample magnitudes.
const FIXTURE_PARAMETERS: &[ParameterSpec] = &[ParameterSpec::new(
    "floor_magnitude",
    ParameterDomain::Integer {
        min: 0,
        max: 32_767,
    },
)];

/// A second noise reducer, implemented from outside `sipx-audio` against the public trait.
///
/// It shares nothing with the baseline but the contract. Where the baseline splits into three
/// bands, tracks a floor and slews a gain, this subtracts one caller-declared magnitude from every
/// sample and keeps no state whatsoever: `y = sign(x)·max(0, |x| − floor_magnitude)`. Its rate list
/// is `Any` where the baseline's is four rates, it accepts mono only where the baseline accepts
/// stereo too, its warm-up is zero where the baseline's is 1,024, it declares `Ignored` where the
/// baseline consumes an activity hint, and it declares `TrustedCooperativeNative` because an
/// application-supplied processor may not call itself proven.
///
/// It is a **worse** noise reducer than the baseline in every audible respect, and that is not the
/// point: the point is that the call layer cannot tell the difference between them, which §4 makes
/// the deliverable of this story.
#[derive(Debug, Clone, Default)]
struct FixedFloorSubtractor {
    admission: FrameAdmission,
    floor_magnitude: i32,
}

impl FixedFloorSubtractor {
    fn declaration() -> DspCapability {
        DspCapability::new("fixture.fixed_floor")
            .with_rates(RateSupport::Any)
            .with_channels(&[1])
            .with_parameters(FIXTURE_PARAMETERS)
            .with_state_bytes(64)
            .with_length(LengthPolicy::Preserving)
            .with_reset(ResetBehavior::Stateless)
            .with_execution(ExecutionPolicy::new(
                ExecutionProfile::TrustedCooperativeNative,
            ))
    }
}

impl FrameProcessor for FixedFloorSubtractor {
    fn capability(&self) -> DspCapability {
        Self::declaration()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        let capability = self.capability();
        capability.validate_parameters(parameters)?;
        for parameter in parameters {
            if let ("floor_magnitude", ParameterValue::Integer(value)) =
                (parameter.id(), parameter.value())
            {
                self.floor_magnitude = i32::try_from(value).unwrap_or(0);
            }
        }
        Ok(())
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
        let mut unchanged = true;
        for sample in frame.samples() {
            let magnitude = i32::from(*sample).abs();
            let reduced = (magnitude - self.floor_magnitude).max(0);
            let signed = if *sample < 0 { -reduced } else { reduced };
            let written = i16::try_from(signed).unwrap_or(i16::MAX);
            unchanged &= written == *sample;
            sink.push(written)?;
        }
        if unchanged {
            sink.emit(DspObservation::PassedThrough);
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

impl NoiseReducer for FixedFloorSubtractor {
    fn noise_reduction(&self) -> NoiseReduction {
        NoiseReduction::new(Self::declaration())
    }
}

// ------------------------------------------------------------- NR-V1..NR-V7: the sample path ----

/// NR-V1: for the whole declared warm-up the baseline is the exact identity, and says so.
///
/// §5.4 is the reason this is bit-exact rather than nearly so: the three bands are defined by
/// subtraction, so `b₀ + b₁ + b₂ = x` whatever the sections rounded to, and an all-unity gain sums
/// them back to the input exactly.
#[test]
fn the_warm_up_is_the_exact_identity() {
    let mut reducer = SubbandSuppressor::new();
    let warm_up = usize::try_from(reducer.noise_reduction().warm_up_positions()).unwrap();
    assert_eq!(warm_up, 1_024, "§5's declared warm-up");

    let input: Vec<i16> = (0..warm_up)
        .map(|n| match n % 4 {
            0 => 1,
            1 => -1,
            2 => i16::MAX,
            _ => i16::MIN,
        })
        .collect();
    let run = replay(&mut reducer, &input, 160);

    assert_eq!(
        run.samples, input,
        "the warm-up is the identity, bit for bit"
    );
    assert!(
        run.observations
            .iter()
            .all(|observation| *observation == DspObservation::PassedThrough),
        "every warm-up frame passed through: {:?}",
        run.observations
    );
}

/// NR-V2: digital silence in, digital silence out — the `env ≤ 0` guard of §5.3.
#[test]
fn silence_is_never_attenuated_into_something_else() {
    let mut reducer = SubbandSuppressor::new();
    let input = vec![0i16; 4_096];
    let run = replay(&mut reducer, &input, 160);

    assert_eq!(run.samples, input);
    assert!(
        run.observations
            .iter()
            .all(|observation| *observation == DspObservation::PassedThrough),
        "silence passes through: {:?}",
        run.observations
    );
}

/// NR-V3: stationary full-scale noise settles at exactly `min_band_gain`.
///
/// At the folding frequency both sections are zero, so `b₂ = x` and the settled output is
/// `scaled(x, 250, 1000)` — one quarter of full scale, and nothing else in the arithmetic.
#[test]
fn stationary_noise_settles_at_the_declared_floor() {
    let mut reducer = SubbandSuppressor::new();
    reducer
        .configure(&[integer("adaptation_positions", 1)])
        .unwrap();
    let input = alternating(4_096, 16_384);
    let run = replay(&mut reducer, &input, 160);

    // 1,024 warm-up positions, then at most 47 slew steps of 16 thousandths each.
    let settled = 1_024 + 47;
    let expected: Vec<i16> = input[settled..]
        .iter()
        .map(|sample| if *sample > 0 { 4_096 } else { -4_096 })
        .collect();
    assert_eq!(run.tail(settled), expected.as_slice());
}

/// NR-V4: a floor of unity is the exact identity for the whole stream, warm-up or not.
#[test]
fn a_unity_floor_is_the_exact_identity() {
    let mut reducer = SubbandSuppressor::new();
    reducer
        .configure(&[
            ratio("min_band_gain", 1_000),
            integer("adaptation_positions", 1),
        ])
        .unwrap();
    let input = alternating(4_096, 16_384);
    let run = replay(&mut reducer, &input, 160);

    assert_eq!(run.samples, input);
}

/// NR-V5: a floor of zero is a hole, on request, and exactly zero rather than nearly.
#[test]
fn a_zero_floor_is_the_hole_it_says_it_is() {
    let mut reducer = SubbandSuppressor::new();
    reducer
        .configure(&[
            ratio("min_band_gain", 0),
            integer("adaptation_positions", 1),
        ])
        .unwrap();
    let input = alternating(4_096, 16_384);
    let run = replay(&mut reducer, &input, 160);

    let settled = 1_024 + 63;
    assert!(
        run.tail(settled).iter().all(|sample| *sample == 0),
        "a zero floor writes silence once the slew has arrived"
    );
}

/// NR-V6: a reset re-opens the warm-up, and the stream after it equals a fresh instance's.
#[test]
fn a_reset_re_opens_the_warm_up() {
    let input = alternating(4_096, 16_384);

    let mut interrupted = SubbandSuppressor::new();
    interrupted
        .configure(&[integer("adaptation_positions", 1)])
        .unwrap();
    let format = d8(&mut interrupted);
    let mut before = Run::default();
    before.feed(&mut interrupted, format, &input).unwrap();
    assert_eq!(interrupted.retained(), 0, "§7: nothing is ever retained");
    interrupted.reset(DspResetCause::Requested);
    assert_eq!(interrupted.retained(), 0);
    let mut after = Run::default();
    after.feed(&mut interrupted, format, &input).unwrap();

    let mut fresh = SubbandSuppressor::new();
    fresh
        .configure(&[integer("adaptation_positions", 1)])
        .unwrap();
    let reference = replay(&mut fresh, &input, 4_096);

    assert_eq!(
        &after.samples[..1_024],
        &input[..1_024],
        "the warm-up re-opened, so the first 1,024 positions are the identity again"
    );
    assert_eq!(
        after.samples, reference.samples,
        "a reset leaves exactly what a fresh instance would produce"
    );
}

/// NR-V7: the optional activity hint freezes the floor, and its absence changes nothing.
///
/// The floor is seeded at the first position of the epoch to half the envelope — `b₂[0]` is
/// `x − s₂[0]` and `s₂[0]` is exactly half of a full-scale first sample — so a frozen floor gives
/// `1000 − 1000·½ = 500` where an adapting one reaches `min_band_gain`.
#[test]
fn the_activity_hint_freezes_the_floor_and_its_absence_does_not() {
    let input = alternating(4_096, 16_384);
    let settled = 1_024 + 63;

    let mut hinted = SubbandSuppressor::new();
    hinted
        .configure(&[
            ratio("over_subtraction", 1_000),
            integer("adaptation_positions", 1),
            flag("voice_active", true),
        ])
        .unwrap();
    let hinted_run = replay(&mut hinted, &input, 160);
    let expected: Vec<i16> = input[settled..]
        .iter()
        .map(|sample| if *sample > 0 { 8_192 } else { -8_192 })
        .collect();
    assert_eq!(
        hinted_run.tail(settled),
        expected.as_slice(),
        "a frozen floor sits at half the envelope, so the gain settles at 0.5"
    );

    let mut unhinted = SubbandSuppressor::new();
    unhinted
        .configure(&[
            ratio("over_subtraction", 1_000),
            integer("adaptation_positions", 1),
        ])
        .unwrap();
    let unhinted_run = replay(&mut unhinted, &input, 160);
    let floor: Vec<i16> = input[settled..]
        .iter()
        .map(|sample| if *sample > 0 { 4_096 } else { -4_096 })
        .collect();
    assert_eq!(
        unhinted_run.tail(settled),
        floor.as_slice(),
        "without the hint the floor reaches the envelope and the gain reaches min_band_gain"
    );
}

// ------------------------------------------------------- NR-V8..NR-V11: refusal and declaration ----

/// NR-V8: a rate outside the declaration is refused, never resampled — §6's first row.
#[test]
fn an_undeclared_format_is_refused_and_never_resampled() {
    let mut reducer = SubbandSuppressor::new();

    let unshaped = StreamFormat::new(44_100, 1).unwrap();
    assert_eq!(
        reducer.prepare(AudioDirection::Inbound, unshaped),
        Err(FormatError::RateNotAccepted { rate: 44_100 })
    );
    let three = StreamFormat::new(8_000, 3).unwrap();
    assert_eq!(
        reducer.prepare(AudioDirection::Inbound, three),
        Err(FormatError::ChannelsNotAccepted { channels: 3 })
    );
    assert!(
        StreamFormat::new(0, 1).is_err(),
        "rate 0 never reaches a reducer: the boundary's own refusal catches it"
    );

    // A refused format leaves nothing half-applied: the declared one still prepares.
    let format = StreamFormat::default();
    assert!(reducer.prepare(AudioDirection::Inbound, format).is_ok());
    for rate in [8_000, 16_000, 32_000, 48_000] {
        let declared = StreamFormat::new(rate, 2).unwrap();
        assert!(
            reducer.prepare(AudioDirection::Inbound, declared).is_ok(),
            "{rate} Hz stereo is declared"
        );
    }
}

/// NR-V9: a refused parameter set leaves the previous one in force, entire.
#[test]
fn a_refused_parameter_set_changes_nothing() {
    let mut reducer = SubbandSuppressor::new();
    reducer
        .configure(&[integer("adaptation_positions", 1)])
        .unwrap();

    assert_eq!(
        reducer.configure(&[ratio("min_band_gain", 1_001)]),
        Err(ParameterError::OutOfRange {
            id: "min_band_gain"
        })
    );
    assert_eq!(
        reducer.configure(&[ratio("over_subtraction", 999)]),
        Err(ParameterError::OutOfRange {
            id: "over_subtraction"
        })
    );
    assert_eq!(
        reducer.configure(&[integer("adaptation_positions", 0)]),
        Err(ParameterError::OutOfRange {
            id: "adaptation_positions"
        })
    );
    assert_eq!(
        reducer.configure(&[integer("voice_active", 1)]),
        Err(ParameterError::KindMismatch { id: "voice_active" })
    );
    assert_eq!(
        reducer.configure(&[flag("denoise_hard", true)]),
        Err(ParameterError::Unknown { id: "denoise_hard" })
    );

    // NR-V3 again, unchanged, which is what "the previous set is still in force" means.
    let input = alternating(4_096, 16_384);
    let run = replay(&mut reducer, &input, 160);
    let settled = 1_024 + 47;
    let expected: Vec<i16> = input[settled..]
        .iter()
        .map(|sample| if *sample > 0 { 4_096 } else { -4_096 })
        .collect();
    assert_eq!(run.tail(settled), expected.as_slice());
}

/// NR-V10: a declaration that lies about its activity input or its warm-up is refused at §3.4.
#[test]
fn an_inadmissible_declaration_is_refused_before_anything_is_sized() {
    let baseline = SubbandSuppressor::new().noise_reduction();
    baseline.validate().expect("the shipped declaration");

    let undeclared = baseline.with_activity_input(ActivityInput::Optional {
        parameter: "not_declared",
    });
    assert_eq!(
        undeclared.validate(),
        Err(NoiseReductionError::ActivityParameterUndeclared {
            parameter: "not_declared"
        })
    );

    let not_a_flag = baseline.with_activity_input(ActivityInput::Optional {
        parameter: "adaptation_positions",
    });
    assert_eq!(
        not_a_flag.validate(),
        Err(NoiseReductionError::ActivityParameterNotAFlag {
            parameter: "adaptation_positions"
        })
    );

    let too_long = baseline.with_warm_up_positions(MAX_WARM_UP_POSITIONS + 1);
    assert_eq!(
        too_long.validate(),
        Err(NoiseReductionError::WarmUp {
            positions: MAX_WARM_UP_POSITIONS + 1
        })
    );

    let anonymous = baseline.with_host_requirement(HostRequirement::Device { id: "" });
    assert_eq!(
        anonymous.validate(),
        Err(NoiseReductionError::EmptyDeviceIdentifier)
    );

    // The processor half is the contract's own refusal, carried rather than re-minted.
    let empty = NoiseReduction::new(DspCapability::new(""));
    assert_eq!(
        empty.validate(),
        Err(NoiseReductionError::Capability(
            CapabilityError::EmptyIdentifier
        ))
    );
}

/// NR-V11: a host requirement is matched against what the caller says it has, never probed.
#[test]
fn a_host_requirement_is_declared_and_matched_never_probed() {
    let portable = SubbandSuppressor::new().noise_reduction();
    assert_eq!(
        portable.host_requirement(),
        HostRequirement::PortableInteger,
        "§5: the baseline needs integer arithmetic and nothing else"
    );
    assert_eq!(portable.admits_host(&[]), Ok(()));

    let accelerated = portable.with_host_requirement(HostRequirement::Device { id: "npu" });
    assert_eq!(
        accelerated.admits_host(&[]),
        Err(NoiseReductionError::DeviceUnavailable { required: "npu" })
    );
    assert_eq!(accelerated.admits_host(&["npu"]), Ok(()));
    assert_eq!(accelerated.admits_host(&["gpu", "npu"]), Ok(()));
}

// ------------------------------------------------- NR-V12..NR-V14: interchangeability itself ----

/// Run one reducer through §11's harness and report which checks it could not prove.
fn conform<P, F>(factory: F) -> Vec<&'static str>
where
    P: FrameProcessor,
    F: FnMut() -> P,
{
    let report = Conformance::new().run(factory);
    assert!(report.passed(), "{report}");
    report
        .unproven()
        .map(sipx_audio::dsp::CheckOutcome::id)
        .collect()
}

/// NR-V12: both reducers pass `DSP-K1`..`DSP-K12`, unproven set exactly `DSP-K9`.
///
/// `DSP-K9` is unproven by construction and not by anything either reducer does: §9.1 of the
/// processor contract reports the heap component of a processor's own state as unproven because
/// `unsafe_code` is forbidden workspace-wide and no counting allocator can be installed. Asserting
/// the set *exactly* is what stops a check quietly slipping from passed to unproven.
#[test]
fn both_reducers_pass_the_same_conformance_run() {
    assert_eq!(conform(SubbandSuppressor::new), [CHECK_ALLOCATION]);
    assert_eq!(conform(FixedFloorSubtractor::default), [CHECK_ALLOCATION]);
}

/// The whole lifecycle, written once, against any reducer at all.
///
/// This function is §4.3 in executable form: it names the trait, reads the declaration for
/// everything it needs to size and to expect, and could not be written differently for one
/// implementation than for the other.
fn lifecycle(reducer: &mut dyn NoiseReducer) {
    let declaration = reducer.noise_reduction();
    declaration.validate().expect("a valid declaration");
    let capability = declaration.processor();
    assert_eq!(capability.length(), LengthPolicy::Preserving);
    assert_eq!(capability.latency_positions(), 0);
    assert_eq!(capability.tail_positions(), 0);
    assert!(capability.scratch_samples() <= 65_536);

    let format = d8(reducer);
    assert_eq!(reducer.retained(), 0, "nothing retained after prepare");

    // A stream long enough to leave any declared warm-up behind.
    let positions = usize::try_from(declaration.warm_up_positions()).unwrap() + 2_048;
    let input = alternating(positions, 12_000);
    let mut run = Run::default();
    for window in input.chunks(160) {
        run.feed(reducer, format, window).expect("a valid frame");
    }
    assert_eq!(
        run.samples.len(),
        input.len(),
        "length policy is preserving"
    );
    assert_eq!(reducer.retained(), 0, "§7: no raw audio is ever retained");

    // §8.3: a refusal writes nothing and mutates nothing.
    let outbound = DspFrame::new(
        AudioDirection::Outbound,
        format,
        run.position,
        &input[..160],
    );
    let mut sink_output = vec![0i16; 160];
    let mut observations = Vec::new();
    let mut scratch = vec![0i16; usize::try_from(capability.scratch_samples()).unwrap()];
    {
        let mut scratch = Scratch::new(&mut scratch);
        let mut sink = FrameSink::new(&mut sink_output, &mut observations, 8);
        assert_eq!(
            reducer.process(&outbound, &mut scratch, &mut sink),
            Err(ProcessError::DirectionMismatch)
        );
        assert_eq!(sink.written(), 0, "a refusal writes nothing");
    }
    assert!(observations.is_empty(), "a refusal enqueues nothing");

    // The stream continues exactly where it stood.
    run.feed(reducer, format, &input[..160])
        .expect("the epoch is where the refusal left it");

    // §8.1: a discontinuity restarts the epoch and is announced.
    let mut broken = Run::default();
    broken
        .feed_at(
            reducer,
            format,
            &input[..160],
            0,
            Some(DiscontinuityKind::Realign),
        )
        .expect("a realigned frame opens a new epoch at 0");
    assert!(
        broken.observations.contains(&DspObservation::Restarted {
            cause: DspResetCause::Discontinuity {
                kind: DiscontinuityKind::Realign
            }
        }) || capability.reset() == ResetBehavior::Stateless,
        "a reducer with sample memory announces the restart it performed"
    );

    // §8.4: cancellation is terminal, idempotent and releases everything.
    reducer.cancel();
    assert_eq!(reducer.retained(), 0);
    reducer.cancel();
    let mut sink_output = vec![0i16; 160];
    let mut observations = Vec::new();
    let mut scratch = vec![0i16; usize::try_from(capability.scratch_samples()).unwrap()];
    let mut scratch = Scratch::new(&mut scratch);
    let mut sink = FrameSink::new(&mut sink_output, &mut observations, 8);
    let frame = DspFrame::new(AudioDirection::Inbound, format, 0, &input[..160]);
    assert_eq!(
        reducer.process(&frame, &mut scratch, &mut sink),
        Err(ProcessError::Cancelled)
    );
    assert_eq!(
        reducer.flush(&mut sink),
        Err(ProcessError::Cancelled),
        "flush after cancel is refused too"
    );
    assert_eq!(sink.written(), 0);
}

/// NR-V13: both reducers complete the same lifecycle, driven by code that names neither.
#[test]
fn one_driver_runs_either_implementation() {
    let mut reducers: Vec<Box<dyn NoiseReducer>> = vec![
        Box::new(SubbandSuppressor::new()),
        Box::new(FixedFloorSubtractor::default()),
    ];
    for reducer in &mut reducers {
        lifecycle(reducer.as_mut());
    }

    // And the declarations really do differ, so this proved substitution rather than symmetry.
    let baseline = SubbandSuppressor::new().noise_reduction();
    let fixture = FixedFloorSubtractor::default().noise_reduction();
    assert_ne!(
        baseline.warm_up_positions(),
        fixture.warm_up_positions(),
        "different warm-ups"
    );
    assert_ne!(
        baseline.processor().reset(),
        fixture.processor().reset(),
        "different reset behaviour"
    );
    assert_ne!(
        baseline.processor().execution().profile(),
        fixture.processor().execution().profile(),
        "different execution profiles"
    );
    assert_eq!(
        baseline.activity_input(),
        ActivityInput::Optional {
            parameter: "voice_active"
        }
    );
    assert_eq!(fixture.activity_input(), ActivityInput::Ignored);
}

/// NR-V14: two instances interleaved frame by frame share no adaptive state.
#[test]
fn two_instances_share_no_adaptive_state() {
    let noisy = alternating(4_096, 16_384);
    let quiet = vec![0i16; 4_096];

    let mut alone_noisy = SubbandSuppressor::new();
    let alone_noisy = replay(&mut alone_noisy, &noisy, 160);
    let mut alone_quiet = SubbandSuppressor::new();
    let alone_quiet = replay(&mut alone_quiet, &quiet, 160);

    let mut first = SubbandSuppressor::new();
    let mut second = SubbandSuppressor::new();
    let format = d8(&mut first);
    d8(&mut second);
    let mut first_run = Run::default();
    let mut second_run = Run::default();
    for (noisy, quiet) in noisy.chunks(160).zip(quiet.chunks(160)) {
        first_run.feed(&mut first, format, noisy).unwrap();
        second_run.feed(&mut second, format, quiet).unwrap();
    }

    assert_eq!(first_run.samples, alone_noisy.samples);
    assert_eq!(second_run.samples, alone_quiet.samples);
}

// ------------------------------------------------------------ NR-V15, NR-V16: the corpus ----

/// The corpus generator of §8: a stated integer recurrence, never a random source.
fn lcg(state: &mut u32) -> i16 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    #[allow(clippy::cast_possible_truncation)]
    {
        (*state >> 16) as i16
    }
}

fn noise(positions: usize, amplitude: i32) -> Vec<i16> {
    let mut state = 1u32;
    (0..positions)
        .map(|_| {
            let raw = i32::from(lcg(&mut state));
            i16::try_from(raw * amplitude / 32_768).unwrap_or(0)
        })
        .collect()
}

fn voice(positions: usize, amplitude: i32, period: usize) -> Vec<i16> {
    (0..positions)
        .map(|n| {
            if (n / 800) % 2 == 1 {
                return 0;
            }
            let phase = i32::try_from(n % period).unwrap();
            let span = i32::try_from(period).unwrap();
            i16::try_from((phase * 2 - span) * amplitude / span).unwrap_or(0)
        })
        .collect()
}

/// The §8 conditions, by name.
fn corpus(condition: &str) -> Vec<i16> {
    match condition {
        "silence" => vec![0i16; 4_096],
        "stationary" => noise(12_288, 2_000),
        "transient" => {
            let mut signal = noise(12_288, 2_000);
            for (offset, sample) in alternating(32, i16::MAX).iter().enumerate() {
                signal[8_192 + offset] = *sample;
            }
            signal
        }
        "overlapping" => {
            let near = voice(12_288, 8_000, 100);
            let far = voice(12_288, 2_000, 137);
            let background = noise(12_288, 800);
            (0..12_288)
                .map(|n| {
                    let sum = i32::from(near[n]) + i32::from(far[n]) + i32::from(background[n]);
                    i16::try_from(sum.clamp(-32_768, 32_767)).unwrap()
                })
                .collect()
        }
        other => panic!("no corpus condition named {other}"),
    }
}

/// FNV-1a over each §8 condition, and the one thing holding two generators to one corpus.
///
/// `crates/sipx-audio/examples/dsp_cost.rs` measures what the reducer *costs* on these four
/// signals; the vectors below measure what it *does* to them. A test helper is not reachable from
/// an example, so the example carries its own copy of §8's recurrences — and a cost figure taken
/// on a signal that had quietly drifted from the one the quality figures describe would be two
/// measurements of two different things, presented as one. These numbers are printed by every
/// `dsp_cost` run under `corpus`, and `X-109` found a real divergence with them: the example
/// spliced `transient` with `i16::MIN` where §8 and this file use `-i16::MAX`.
const CORPUS_CHECKSUMS: &[(&str, u64)] = &[
    ("silence", 0xb93a_0c83_ce3b_6325),
    ("stationary", 0x8934_3e5d_b98e_8306),
    ("transient", 0xe980_f030_62d9_50a3),
    ("overlapping", 0x05b2_2294_c65e_ee88),
];

fn checksum(samples: &[i16]) -> u64 {
    samples.iter().fold(0xcbf2_9ce4_8422_2325, |hash, sample| {
        (hash ^ u64::from(u16::from_ne_bytes(sample.to_ne_bytes())))
            .wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// NR-V23 (`X-109`) — the corpus is these exact samples, and the cost harness measures the same
/// ones.
#[test]
fn the_corpus_is_the_same_corpus_the_cost_harness_measures() {
    for (name, expected) in CORPUS_CHECKSUMS {
        let signal = corpus(name);
        assert_eq!(checksum(&signal), *expected, "{name}");
    }
    assert_eq!(corpus("silence").len(), 4_096);
    assert_eq!(corpus("stationary").len(), 12_288);
}

/// Summed magnitude of the output over the input's, in thousandths, after the warm-up (§8).
fn attenuation(input: &[i16], output: &[i16], from: usize) -> i64 {
    let sum = |samples: &[i16]| -> i64 {
        samples[from..]
            .iter()
            .map(|sample| i64::from(*sample).abs())
            .sum()
    };
    let reference = sum(input);
    if reference == 0 {
        return 1_000;
    }
    sum(output) * 1_000 / reference
}

/// Summed magnitude of `output − input` over the input's, in thousandths, after the warm-up (§8).
fn distortion(input: &[i16], output: &[i16], from: usize) -> i64 {
    let reference: i64 = input[from..]
        .iter()
        .map(|sample| i64::from(*sample).abs())
        .sum();
    if reference == 0 {
        return 0;
    }
    let error: i64 = input[from..]
        .iter()
        .zip(&output[from..])
        .map(|(had, got)| (i64::from(*got) - i64::from(*had)).abs())
        .sum();
    error * 1_000 / reference
}

/// NR-V15: the predeclared corpus, measured — and bounded by an arithmetic reason, not a taste.
#[test]
fn the_predeclared_corpus_measures_what_the_spec_says_it_does() {
    let warm_up = 1_024;

    for condition in ["silence", "stationary", "transient", "overlapping"] {
        let input = corpus(condition);
        let mut reducer = SubbandSuppressor::new();
        let run = replay(&mut reducer, &input, 160);
        assert_eq!(run.samples.len(), input.len());

        let kept = attenuation(&input, &run.samples, warm_up);
        let damage = distortion(&input, &run.samples, warm_up);
        assert!(
            (250..=1_000).contains(&kept),
            "{condition}: `min_band_gain` bounds the attenuation below and unity bounds it above, \
             measured {kept}"
        );

        // The recorded figures. Each is locked exactly so that a change to the arithmetic moves a
        // number rather than a judgement, and each is accompanied by the reason it is where it is.
        let expected = match condition {
            // Nothing to attenuate and nothing to damage.
            "silence" => (1_000, 0),
            // The whole signal is noise, so every band settles at the floor it declared and the
            // "distortion" is the noise removal itself.
            "stationary" => (298, 707),
            // A transient survives *better* than the stationary noise around it — §5.6's second
            // row: a full-scale burst is faster than the floor creeps, so it passes through and
            // then raises the floor for what follows.
            "transient" => (377, 643),
            // The near talker rides above the floor the background pulled up, so most of the
            // speech survives — and 177 thousandths of it does not (§5.6).
            "overlapping" => (859, 177),
            other => panic!("no corpus condition named {other}"),
        };
        assert_eq!(
            (kept, damage),
            expected,
            "{condition}: attenuation and distortion, in thousandths"
        );
    }

    // The orderings above are the spec's claims and not an artefact of one run's numbers.
    let measure = |condition: &str| {
        let input = corpus(condition);
        let mut reducer = SubbandSuppressor::new();
        let run = replay(&mut reducer, &input, 160);
        attenuation(&input, &run.samples, warm_up)
    };
    assert!(
        measure("transient") > measure("stationary"),
        "a transient is not attenuated as the noise around it is"
    );
    assert!(
        measure("overlapping") > measure("transient"),
        "speech above the floor survives where noise at it does not"
    );
}

/// NR-V17: onset recovery is the gain slew's length and nothing else.
///
/// A step from noise-level to speech-level input, after the gain has settled at `min_band_gain`.
/// The measure is §8's: the positions until the output is within one sixteenth of the input.
#[test]
fn onset_recovery_is_the_slew_it_declared() {
    let quiet = alternating(4_096, 500);
    let loud = alternating(1_024, 16_000);
    let mut input = quiet.clone();
    input.extend_from_slice(&loud);

    // Default parameters, deliberately: `adaptation_positions = 1` — which the sample-exact
    // vectors above use to reach a settled state in a handful of positions — makes the floor
    // follow the envelope almost instantly, at which point every signal is its own noise floor,
    // nothing ever recovers, and the suppressor holds everything at `min_band_gain`. That is a
    // real property of the arithmetic and the reason that setting is a test lever and not a call
    // configuration.
    let mut reducer = SubbandSuppressor::new();
    let run = replay(&mut reducer, &input, 160);

    let onset = quiet.len();
    let recovered = (onset..input.len())
        .find(|n| {
            let had = i64::from(input[*n]).abs();
            let got = i64::from(run.samples[*n]).abs();
            got * 16 >= had * 15
        })
        .expect("the output recovers to within a sixteenth of the input");
    let positions = recovered - onset;

    // Hand-derivable: the gain sits at `min_band_gain` when the onset arrives, one sixteenth of
    // the input needs 938 thousandths, and the slew moves 16 per position — so the gain at the
    // 43rd step after the onset is 250 + 16·43 = 938, which is the position at index `onset + 42`.
    // Onset recovery is the gain slew's length and nothing else, counted in positions.
    assert_eq!(positions, 42, "onset recovery, in positions");
    assert!(
        positions < 64,
        "recovery is bounded by the declared slew, measured {positions}"
    );
}

/// NR-V16: the same stream produces the same samples however the caller cut it.
#[test]
fn the_corpus_is_independent_of_framing() {
    let input = corpus("stationary");
    let mut whole = SubbandSuppressor::new();
    let reference = replay(&mut whole, &input, input.len());

    for chunk in [1, 7, 13, 160, 4_096] {
        let mut reducer = SubbandSuppressor::new();
        let run = replay(&mut reducer, &input, chunk);
        assert_eq!(run.samples, reference.samples, "cut at {chunk}");
    }
}

// ------------------------------------------ NR-V18..NR-V22: the activity-hint producer (§10) ----

/// What one hinted replay accounted for, beside the samples it produced.
#[derive(Debug, Default)]
struct HintTally {
    changes: Vec<(u64, bool, HintCause)>,
    hinted_positions: u64,
    unprotected: u64,
    overheld: u64,
}

/// §10.2's placement, driven: one seam frame reaches the analyser and the reducer, and the hint a
/// frame's observations produce takes effect at the boundary **after** that frame.
///
/// The reducer is fed first and the analyser second on purpose. Feeding the analyser first would
/// let a hint derived from a frame's own samples reach the reducer before those samples do, which
/// is a lookahead the reducer's declared zero latency says it does not have.
fn replay_hinted(
    reducer: &mut dyn NoiseReducer,
    input: &[i16],
    chunk: usize,
    policy: HintPolicy,
) -> (Run, HintTally) {
    let format = d8(reducer);
    let mut analyzer =
        AudioAnalyzer::new(AnalysisProfile::new(AudioDirection::Inbound, 8_000)).unwrap();
    let mut hint = ActivityHint::new(AudioDirection::Inbound, reducer.noise_reduction(), policy);
    let mut run = Run::default();
    let mut tally = HintTally::default();
    let mut applied = false;

    for (sequence, window) in input.chunks(chunk).enumerate() {
        if hint.voice_active() != applied {
            let parameter = hint
                .parameter()
                .expect("the baseline declares an activity parameter");
            reducer
                .configure(&[parameter])
                .expect("the declared flag is in the declared schema");
            applied = hint.voice_active();
        }
        if applied {
            tally.hinted_positions += u64::try_from(window.len()).unwrap();
        }
        run.feed(reducer, format, window).expect("a valid frame");

        let frame = AnalysisFrame::new(
            AudioDirection::Inbound,
            u64::try_from(sequence).unwrap(),
            window,
        );
        analyzer.process(&frame).expect("a valid analysis frame");
        let at = run.position;
        for observation in analyzer.drain() {
            if let Some(change) = hint.observe(at, &observation) {
                tally
                    .changes
                    .push((change.at_position(), change.voice_active(), change.cause()));
            }
        }
        if let Some(change) = hint.advance_to(at) {
            tally
                .changes
                .push((change.at_position(), change.voice_active(), change.cause()));
        }
    }

    tally.unprotected = hint.unprotected_positions();
    tally.overheld = hint.overheld_positions();
    (run, tally)
}

/// NR-V18: the producer reads the reducer's declaration and sets what it names, or nothing.
#[test]
fn the_producer_sets_the_parameter_the_declaration_names() {
    let reducer = SubbandSuppressor::new();
    let declared = reducer.noise_reduction();
    assert_eq!(
        declared.activity_input(),
        ActivityInput::Optional {
            parameter: "voice_active"
        }
    );

    let hint = ActivityHint::new(AudioDirection::Inbound, declared, HintPolicy::new());
    assert_eq!(hint.direction(), AudioDirection::Inbound);
    assert_eq!(hint.parameter_id(), Some("voice_active"));
    assert_eq!(hint.parameter(), Some(flag("voice_active", false)));
    assert!(
        !hint.voice_active(),
        "a producer that has seen nothing hints nothing"
    );
    assert_eq!(
        hint.warm_up_positions(),
        u64::from(declared.warm_up_positions()),
        "the deferral is the reducer's own declared count, never a number the policy invented"
    );

    // §3.3: a caller wiring a detector to `Ignored` is wiring it to nothing, and the producer says
    // so rather than inventing a parameter name nobody declared.
    let deaf = ActivityHint::new(
        AudioDirection::Inbound,
        NoiseReduction::new(DspCapability::new("deaf")),
        HintPolicy::new(),
    );
    assert_eq!(deaf.parameter_id(), None);
    assert_eq!(deaf.parameter(), None);
}

/// A reducer declaration that consumes the hint and warms up instantly.
///
/// §10.2's deferral is measured separately, by `the_corpus_is_measured_with_the_hint_wired_and_unwired`
/// against the baseline's real 1,024 positions; the vector below is the policy's own arithmetic
/// with that one term removed, so each edge is hand-derivable from the positions beside it.
const HINT_SCHEMA: &[ParameterSpec] = &[ParameterSpec::new("voice_active", ParameterDomain::Flag)];

fn undeferred() -> NoiseReduction {
    NoiseReduction::new(DspCapability::new("undeferred").with_parameters(HINT_SCHEMA))
        .with_activity_input(ActivityInput::Optional {
            parameter: "voice_active",
        })
}

/// NR-V19: the policy, sample-exact, on a stated observation sequence.
///
/// Positions are the caller's, in samples, and every edge below is hand-derivable from them: the
/// analyser's own `W` and hangover are what make the two lags what they are, and the hold cap is
/// what recovers a hint whose closing transition never arrived.
#[test]
fn the_policy_is_a_stated_function_of_positions() {
    let policy = HintPolicy::new()
        .with_release_positions(0)
        .with_hold_positions(4_000);
    let declared = undeferred();
    let mut hint = ActivityHint::new(AudioDirection::Inbound, declared, policy);

    // A window that completed at position 160 says voice began at 0: the hint is late by exactly
    // the detector's window, and the producer accounts for those 160 positions rather than hiding
    // them.
    let started = hint
        .observe(160, &Observation::VoiceStarted { at_sample: 0 })
        .expect("the leading edge is a change");
    assert_eq!(started.at_position(), 160);
    assert!(started.voice_active());
    assert_eq!(started.lag_positions(), 160);
    assert_eq!(started.cause(), HintCause::VoiceStarted);
    assert_eq!(hint.unprotected_positions(), 160);
    assert_eq!(hint.parameter(), Some(flag("voice_active", true)));

    // An active window refreshes the hold and is not a second edge.
    assert_eq!(hint.observe(320, &active_window(1)), None);
    assert_eq!(hint.advance_to(320), None);

    // Nothing refreshes it again: the cap fires 4,000 positions after the last refresh at 320.
    assert_eq!(hint.advance_to(4_319), None);
    let elapsed = hint
        .advance_to(4_320)
        .expect("an unrefreshed hold is bounded");
    assert!(!elapsed.voice_active());
    assert_eq!(elapsed.cause(), HintCause::HoldElapsed);
    assert_eq!(elapsed.at_position(), 4_320);
    assert_eq!(hint.parameter(), Some(flag("voice_active", false)));

    // A lost observation may have been the closing transition, and NR-V21 measures that direction
    // at 420 thousandths against the other's 4 — so the hole resolves toward not hinting.
    let mut hint = ActivityHint::new(AudioDirection::Inbound, declared, policy);
    hint.observe(160, &Observation::VoiceStarted { at_sample: 0 });
    let lost = hint
        .observe(480, &Observation::Lost { count: 3 })
        .expect("a hole in the stream is a change");
    assert!(!lost.voice_active());
    assert_eq!(lost.cause(), HintCause::ObservationsLost);
    assert_eq!(hint.advance_to(640), None, "and it does not re-arm itself");

    // The closing transition arrives 1,600 positions — the reference hangover — after the speech it
    // describes ended, and the producer counts that over-hold too.
    let mut hint = ActivityHint::new(AudioDirection::Inbound, declared, policy);
    hint.observe(160, &Observation::VoiceStarted { at_sample: 0 });
    let ended = hint
        .observe(
            3_200,
            &Observation::VoiceEnded {
                at_sample: 1_600,
                cause: VoiceEndCause::Hangover,
            },
        )
        .expect("the trailing edge is a change");
    assert!(!ended.voice_active());
    assert_eq!(ended.cause(), HintCause::VoiceEnded);
    assert_eq!(ended.lag_positions(), 1_600);
    assert_eq!(hint.overheld_positions(), 1_600);

    // A release guard delays the clear by the positions it declares, and by no others.
    let guarded = HintPolicy::new().with_release_positions(240);
    let mut hint = ActivityHint::new(AudioDirection::Inbound, declared, guarded);
    hint.observe(160, &Observation::VoiceStarted { at_sample: 0 });
    assert_eq!(
        hint.observe(
            3_200,
            &Observation::VoiceEnded {
                at_sample: 1_600,
                cause: VoiceEndCause::Hangover,
            },
        ),
        None,
        "the guard holds the hint past the transition"
    );
    assert!(hint.voice_active());
    assert_eq!(hint.advance_to(3_439), None);
    let released = hint.advance_to(3_440).expect("the guard elapses");
    assert_eq!(released.cause(), HintCause::VoiceEnded);
    assert_eq!(released.lag_positions(), 1_840);

    // An epoch that restarted has no activity to describe, and the reducer's warm-up re-opens with
    // it (§5.4): the hint clears at the reset and re-arms only at the next transition.
    let mut hint = ActivityHint::new(AudioDirection::Inbound, declared, policy);
    hint.observe(160, &Observation::VoiceStarted { at_sample: 0 });
    let cut = hint
        .observe(
            320,
            &Observation::Reset {
                cause: ResetCause::Requested,
            },
        )
        .expect("a reset clears the hint");
    assert!(!cut.voice_active());
    assert_eq!(cut.cause(), HintCause::EpochReset);
    assert_eq!(hint.advance_to(0), None, "positions restart with the epoch");
}

/// One completed active window, as the analyser spells it (§5.3).
fn active_window(index: u64) -> Observation {
    Observation::Window {
        index,
        peak: 8_192,
        sum: 0,
        energy: 160 * 8_192 * 8_192,
        clipped: 0,
        clipping: false,
        impulsive: false,
        active: true,
        dc_offset: false,
        silent: false,
    }
}

/// §10.5's fifth condition: `overlapping`'s near talker with its gate held on.
///
/// §8's four rows are `M-66`'s and stay four. This one is `M-114`'s, generated by the same
/// recurrences, and it exists because §5.6's fifth row reasons about *continuous* speech and §8
/// has none: `overlapping` gates its talkers off for 800 positions at a time, which is exactly the
/// pause the estimator needs to find a minimum in.
fn sustained() -> Vec<i16> {
    let background = noise(12_288, 800);
    (0..12_288usize)
        .map(|n| {
            let phase = i32::try_from(n % 100).unwrap();
            let near = (phase * 2 - 100) * 8_000 / 100;
            i16::try_from((near + i32::from(background[n])).clamp(-32_768, 32_767)).unwrap()
        })
        .collect()
}

/// NR-V20: §8's corpus and §10.5's fifth condition, measured with the producer wired and without it.
///
/// The comparison is the whole point of the story: a hint is only worth wiring if the numbers move
/// the right way on the condition it was reasoned about, and §5.6's third row is that condition.
/// Two of the five rows below move the wrong way, and they are asserted just as exactly as the one
/// that moves the right way — a measurement that only records its wins is not a measurement.
#[test]
fn the_corpus_is_measured_with_the_hint_wired_and_unwired() {
    let warm_up = 1_024;

    for condition in [
        "silence",
        "stationary",
        "transient",
        "overlapping",
        "sustained",
    ] {
        let input = if condition == "sustained" {
            sustained()
        } else {
            corpus(condition)
        };

        let mut unwired = SubbandSuppressor::new();
        let plain = replay(&mut unwired, &input, 160);

        let mut wired = SubbandSuppressor::new();
        let (hinted, tally) = replay_hinted(&mut wired, &input, 160, HintPolicy::new());
        assert_eq!(hinted.samples.len(), input.len());

        let measured = (
            attenuation(&input, &plain.samples, warm_up),
            distortion(&input, &plain.samples, warm_up),
            attenuation(&input, &hinted.samples, warm_up),
            distortion(&input, &hinted.samples, warm_up),
            tally.hinted_positions,
            tally.unprotected,
            tally.overheld,
        );

        // Unwired attenuation and distortion, then wired, then the positions the hint covered and
        // the two edges it could not. Each is exact so that a change moves a number rather than a
        // judgement, and each row carries the reason it reads as it does.
        let expected = match condition {
            // Nothing rises above the activation threshold, so the hint is never set.
            "silence" => (1_000, 0, 1_000, 0, 0, 0, 0),
            // The same: 8,000 Hz noise at amplitude 2,000 has a window deviation near 1,155, well
            // under the reference profile's activation amplitude of 2,048.
            "stationary" => (298, 707, 298, 707, 0, 0, 0),
            // A **false active**, produced by the shipped detector on stated input rather than
            // asserted: a 32-position full-scale burst carries a whole window past the activation
            // threshold, and the hint then freezes the floor for the hangover that follows. The
            // reducer removes *less* of the noise it exists to remove — 377 to 397 — which is the
            // cost of a wrong hint, measured.
            "transient" => (377, 643, 397, 631, 1_600, 160, 1_600),
            // The condition the wiring exists for (§5.6, third row), and the only one where both
            // measures move the right way: 4 thousandths more noise removed and 4 thousandths less
            // speech damaged. It is a small margin and it is the whole of the case for wiring this.
            "overlapping" => (859, 177, 855, 173, 11_168, 1_120, 0),
            // A **correct** hint that still costs 15 thousandths of speech. A sawtooth's envelope
            // dips every period, so the unwired floor keeps finding minima; freezing it at 1,120
            // locks in one that is already higher. Nothing in the policy can see this — the arrow
            // runs one way (§10) and the reducer's floor is not readable from here.
            "sustained" => (611, 392, 594, 407, 11_168, 1_120, 0),
            other => panic!("no condition named {other}"),
        };
        assert_eq!(
            measured, expected,
            "{condition}: unwired (attenuation, distortion), wired (attenuation, distortion), \
             hinted positions, unprotected positions, overheld positions"
        );

        if tally.hinted_positions == 0 {
            assert_eq!(
                hinted.samples, plain.samples,
                "{condition}: a stream the detector never calls voice is the unwired stream, \
                 sample for sample and not measure for measure"
            );
        }
    }
}

/// NR-V21: what a wrong hint costs, in each of the two directions §5.6's fifth row names.
///
/// The two numbers this produces — 416 against 4 — are what every uncertainty in §10.3's policy is
/// resolved by. A hint held past the speech it describes is expensive; a hint dropped during speech
/// is nearly free. So the producer clears on doubt, defers through the warm-up and adds nothing to
/// the hangover.
#[test]
fn a_wrong_hint_costs_what_the_spec_says_it_costs() {
    let warm_up = 1_024;
    let input = corpus("overlapping");

    // Wrongly **true**, from the first frame: the floor freezes on the seed §5.2 takes from the
    // first position of the epoch, that seed was taken from speech, and the speech is then
    // attenuated by a floor that describes it. This is the failure §5.4's warm-up exists to
    // prevent, reached by setting the hint inside the warm-up.
    let mut pinned = SubbandSuppressor::new();
    pinned.configure(&[flag("voice_active", true)]).unwrap();
    let pinned_run = replay(&mut pinned, &input, 160);

    // Wrongly **false**, throughout: the unwired run *is* that hint, so the cost is its distance
    // from the run the analyser's own observations produced.
    let mut unwired = SubbandSuppressor::new();
    let unwired_run = replay(&mut unwired, &input, 160);
    let mut wired = SubbandSuppressor::new();
    let (wired_run, _) = replay_hinted(&mut wired, &input, 160, HintPolicy::new());

    let damage = |samples: &[i16]| distortion(&input, samples, warm_up);
    assert_eq!(
        (
            damage(&wired_run.samples),
            damage(&unwired_run.samples),
            damage(&pinned_run.samples)
        ),
        (173, 177, 593),
        "speech distortion in thousandths: hinted, hint wrongly absent, hint wrongly pinned"
    );
    assert_eq!(
        (
            damage(&unwired_run.samples) - damage(&wired_run.samples),
            damage(&pinned_run.samples) - damage(&wired_run.samples),
        ),
        (4, 420),
        "the two directions are not symmetric, and the policy is built around which is which"
    );
}

/// NR-V22: a wrong or absent hint degrades to the unhinted behaviour, never to a refusal — and no
/// observation the reducer produces presents it as a detector.
#[test]
fn a_wrong_or_absent_hint_degrades_rather_than_refuses() {
    let input = corpus("overlapping");

    // Absent: the producer never consulted, the run is `M-66`'s exactly.
    let mut absent = SubbandSuppressor::new();
    let reference = replay(&mut absent, &input, 160);

    // A hint that is wrong every other frame — the detector that is wrong half the time §5.5 warns
    // about. Every frame is still carried and every set is still accepted.
    let mut thrashed = SubbandSuppressor::new();
    let format = d8(&mut thrashed);
    let mut run = Run::default();
    for (index, window) in input.chunks(160).enumerate() {
        thrashed
            .configure(&[flag("voice_active", index % 2 == 0)])
            .expect("the declared flag is always in the declared schema");
        run.feed(&mut thrashed, format, window)
            .expect("a wrong hint is never a refusal");
    }
    assert_eq!(run.samples.len(), reference.samples.len());
    assert_ne!(
        run.samples, reference.samples,
        "the hint changes what the reducer does, which is why a wrong one costs anything"
    );

    // A producer whose observation stream develops a hole degrades to the unhinted behaviour rather
    // than to anything the caller has to handle.
    let mut hint = ActivityHint::new(
        AudioDirection::Inbound,
        SubbandSuppressor::new().noise_reduction(),
        HintPolicy::new(),
    );
    hint.observe(1_120, &Observation::VoiceStarted { at_sample: 0 });
    assert!(hint.voice_active());
    assert_eq!(
        hint.observe(1_280, &Observation::Lost { count: 2 })
            .map(|change| change.cause()),
        Some(HintCause::ObservationsLost)
    );
    assert_eq!(hint.parameter(), Some(flag("voice_active", false)));
    assert!(!hint.voice_active(), "which is `M-66`'s unhinted behaviour");

    // Nothing the reducer emitted claims an activity fact of its own (§2). Every observation it
    // produced is the processor contract's own vocabulary about a parameter or a pass-through.
    for observation in &run.observations {
        let rendered = format!("{observation:?}");
        assert!(
            !rendered.contains("Voice") && !rendered.contains("Activity"),
            "a reducer never presents itself as a detector: {rendered}"
        );
    }
}

/// The registry names exactly what ships, and each name is the one the reducer itself declares.
#[test]
fn the_shipped_reducer_is_the_one_the_list_names() {
    assert_eq!(NOISE_REDUCTION_IDS, [SUBBAND_SUPPRESSOR]);
    let reducer = SubbandSuppressor::new();
    assert_eq!(reducer.capability().id(), SUBBAND_SUPPRESSOR);
    assert_eq!(
        reducer.capability().execution().profile(),
        ExecutionProfile::ProvenInline,
        "the workspace baseline is crate-owned and runs inline"
    );
    assert_eq!(
        reducer.capability().rates(),
        RateSupport::Exactly(&[8_000, 16_000, 32_000, 48_000])
    );
    let parameters: Vec<&str> = reducer
        .capability()
        .parameters()
        .iter()
        .map(ParameterSpec::id)
        .collect();
    assert_eq!(
        parameters,
        [
            "min_band_gain",
            "over_subtraction",
            "adaptation_positions",
            "gain_slew_positions",
            "voice_active",
        ]
    );
}
