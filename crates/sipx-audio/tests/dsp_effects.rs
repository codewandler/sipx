//! The built-in effects and filters seen from outside the crate:
//! `docs/specs/call-dsp-effects.md` (`M-65`).
//!
//! Every processor exercised here is reached through `sipx_audio::dsp`'s **public** API and nothing
//! else, which is the epic's central claim made checkable: `docs/specs/custom-call-dsp.md` §10 says
//! built-in and application-supplied processors use the same interface and that there is no
//! crate-private door, and a built-in tested through an in-crate handle would prove the opposite.
//!
//! The vectors are sample-exact and hand-derivable. The filter vectors run at a cutoff of one
//! quarter of the sample rate, where the shipped coefficient is exactly 0.5 and the one-pole
//! section collapses to a two-tap average — so the impulse, step and frequency responses are
//! arithmetic anyone can check on paper rather than numbers taken from the implementation.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_audio::analysis::{AudioDirection, DiscontinuityKind};
use sipx_audio::dsp::effects::{
    BUILT_IN_IDS, BitCrush, Gain, HardClip, HighPass, LowPass, MAX_STUTTER_POSITIONS, Peaking,
    Polarity, SoftClip, Stutter,
};
use sipx_audio::dsp::{
    CHECK_ALLOCATION, Conformance, DspCapability, DspFrame, DspObservation, DspResetCause,
    ExecutionProfile, FrameProcessor, FrameSink, LengthPolicy, Parameter, ParameterDomain,
    ParameterValue, ProcessError, ResetBehavior, Scratch, StreamFormat,
};

// ------------------------------------------------------------------------- the driver ----

/// One replay's accumulated evidence: what the processor wrote and what it said about it.
#[derive(Debug, Default)]
struct Run {
    samples: Vec<i16>,
    observations: Vec<DspObservation>,
    position: u64,
}

impl Run {
    /// Offer one frame, continuing this replay's epoch.
    fn feed<P: FrameProcessor>(
        &mut self,
        processor: &mut P,
        format: StreamFormat,
        input: &[i16],
    ) -> Result<(), ProcessError> {
        self.feed_at(processor, format, input, self.position, None)
    }

    /// Offer one frame at a stated position, optionally carrying a declared break.
    fn feed_at<P: FrameProcessor>(
        &mut self,
        processor: &mut P,
        format: StreamFormat,
        input: &[i16],
        position: u64,
        discontinuity: Option<DiscontinuityKind>,
    ) -> Result<(), ProcessError> {
        let capability = processor.capability();
        let channels = usize::from(format.channels());
        let positions = u32::try_from(input.len() / channels).unwrap();
        let mut output = vec![0i16; capability.max_output_positions(positions) as usize * channels];
        let mut scratch_buffer = vec![0i16; capability.scratch_samples() as usize];
        let mut observations = Vec::new();

        let mut frame = DspFrame::new(AudioDirection::Inbound, format, position, input);
        if let Some(kind) = discontinuity {
            frame = frame.with_discontinuity(kind);
        }
        let written = {
            let mut scratch = Scratch::new(&mut scratch_buffer);
            let mut sink = FrameSink::new(&mut output, &mut observations, 32);
            processor.process(&frame, &mut scratch, &mut sink)?;
            sink.written()
        };
        self.samples.extend_from_slice(&output[..written]);
        self.observations.extend(observations);
        self.position = position + u64::from(positions);
        Ok(())
    }

    /// Drain the processor's tail into this replay.
    fn flush<P: FrameProcessor>(&mut self, processor: &mut P, format: StreamFormat) {
        let capability = processor.capability();
        let channels = usize::from(format.channels());
        let mut output = vec![0i16; capability.tail_positions() as usize * channels];
        let mut observations = Vec::new();
        let written = {
            let mut sink = FrameSink::new(&mut output, &mut observations, 32);
            processor.flush(&mut sink).unwrap();
            sink.written()
        };
        self.samples.extend_from_slice(&output[..written]);
        self.observations.extend(observations);
    }
}

/// The narrowband reference stream every vector below runs on unless it says otherwise.
fn d8() -> StreamFormat {
    StreamFormat::new(8_000, 1).unwrap()
}

/// Prepare a processor on `D8`, apply a parameter set, and hand back a replay to drive it with.
fn prepared<P: FrameProcessor>(processor: &mut P, parameters: &[Parameter]) -> Run {
    processor.configure(parameters).unwrap();
    processor.prepare(AudioDirection::Inbound, d8()).unwrap();
    Run::default()
}

/// One whole replay in one frame.
fn once<P: FrameProcessor>(processor: &mut P, parameters: &[Parameter], input: &[i16]) -> Run {
    let mut run = prepared(processor, parameters);
    run.feed(processor, d8(), input).unwrap();
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

// ------------------------------------------------------------------- conformance (K1..K12) ----

/// Run one built-in through §11's harness and report which checks it could not prove.
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

/// Acceptance: each built-in passes the external-processor conformance harness.
///
/// `DSP-K9` is the one check that comes back `Unproven` for every processor, built-in or not, and
/// it is unproven by construction rather than by anything these processors do: §9.1 reports the
/// heap component of a processor's own state as unproven because `unsafe_code` is forbidden
/// workspace-wide and no counting allocator can be installed. Asserting the unproven set *exactly*
/// is what keeps that a stated fact instead of a check quietly slipping from passed to unproven.
#[test]
fn every_built_in_passes_the_conformance_harness() {
    assert_eq!(conform(Gain::new), [CHECK_ALLOCATION]);
    assert_eq!(conform(Polarity::new), [CHECK_ALLOCATION]);
    assert_eq!(conform(HardClip::new), [CHECK_ALLOCATION]);
    assert_eq!(conform(SoftClip::new), [CHECK_ALLOCATION]);
    assert_eq!(conform(BitCrush::new), [CHECK_ALLOCATION]);
    assert_eq!(conform(LowPass::new), [CHECK_ALLOCATION]);
    assert_eq!(conform(HighPass::new), [CHECK_ALLOCATION]);
    assert_eq!(conform(Peaking::new), [CHECK_ALLOCATION]);
    for delay in [0, 1, 4, MAX_STUTTER_POSITIONS] {
        assert_eq!(
            conform(|| Stutter::new(delay).unwrap()),
            [CHECK_ALLOCATION],
            "delay {delay}"
        );
    }
}

/// Acceptance: every processor publishes closed parameter ranges, supported rates and channels,
/// latency, tail, reset and smoothing behaviour through `M-63` capability discovery.
///
/// Smoothing is published the same way everything else is — through the declared schema. A
/// processor that ramps a parameter change declares `smoothing_positions` and its closed range; a
/// processor that applies a change whole declares no such parameter, and the absence is as
/// discoverable as the presence.
#[test]
fn every_built_in_declares_a_closed_schema() {
    let declarations: Vec<(DspCapability, &[&str])> = vec![
        (Gain::new().capability(), &["gain", "smoothing_positions"]),
        (Polarity::new().capability(), &["inverted"]),
        (HardClip::new().capability(), &["ceiling"]),
        (SoftClip::new().capability(), &["threshold"]),
        (BitCrush::new().capability(), &["bits", "hold_positions"]),
        (Stutter::new(4).unwrap().capability(), &["repeat"]),
        (LowPass::new().capability(), &["cutoff_hz"]),
        (HighPass::new().capability(), &["cutoff_hz"]),
        (Peaking::new().capability(), &["centre_hz", "band_gain"]),
    ];
    let mut ids = Vec::new();
    for (capability, parameters) in &declarations {
        capability.validate().unwrap();
        ids.push(capability.id());

        // Every built-in is crate-owned and runs inline on the media worker under the profile
        // whose containment is in the shape of its work.
        assert_eq!(
            capability.execution().profile(),
            ExecutionProfile::ProvenInline,
            "{}",
            capability.id()
        );
        // §4.3 of the graph contract: a live call's packetisation is fixed, so every stage that
        // may reach it preserves its frame's position count.
        assert_eq!(
            capability.length(),
            LengthPolicy::Preserving,
            "{}",
            capability.id()
        );
        // Mono and stereo: the call paths this epic serves. Eight channels are the contract's
        // headroom, and nothing here measures them.
        assert_eq!(capability.channels(), [1, 2], "{}", capability.id());

        let declared: Vec<&str> = capability
            .parameters()
            .iter()
            .map(sipx_audio::dsp::ParameterSpec::id)
            .collect();
        assert_eq!(declared.as_slice(), *parameters, "{}", capability.id());
        // Closed on both ends, and never a floating-point domain — §3.6's vocabulary cannot
        // express one.
        for spec in capability.parameters() {
            match spec.domain() {
                ParameterDomain::Flag => {}
                ParameterDomain::Integer { min, max } => assert!(min <= max, "{}", spec.id()),
                ParameterDomain::Ratio { min, max } => assert!(min <= max, "{}", spec.id()),
                _ => panic!("{} declares a domain outside §3.6", spec.id()),
            }
        }
    }
    // The shipped identifiers are the registry's, in declaration order.
    assert_eq!(ids, BUILT_IN_IDS);
}

// --------------------------------------------------------------------------- gain ----

/// EFFECT-V1: a gain of 2.0 doubles, clamps at full scale and counts the clamped positions.
#[test]
fn gain_scales_and_saturates() {
    let mut gain = Gain::new();
    let run = once(
        &mut gain,
        &[ratio("gain", 2_000)],
        &[1, -1, 16_384, -16_384, 32_767, -32_768],
    );
    assert_eq!(run.samples, [2, -2, 32_767, -32_768, 32_767, -32_768]);
    assert_eq!(
        run.observations,
        [
            DspObservation::ParameterApplied {
                parameter: "gain",
                at_position: 0
            },
            DspObservation::Saturated { positions: 3 },
        ]
    );
}

/// EFFECT-V2: a fractional gain rounds half away from zero, symmetrically about zero.
#[test]
fn gain_rounds_half_away_from_zero() {
    let mut gain = Gain::new();
    let run = once(&mut gain, &[ratio("gain", 1_500)], &[1, 2, 3, -1, -2, -3]);
    assert_eq!(run.samples, [2, 3, 5, -2, -3, -5]);
}

/// EFFECT-V3: unity gain is the identity and says so.
#[test]
fn unity_gain_passes_through() {
    let mut gain = Gain::new();
    let run = once(&mut gain, &[], &[0, 1, -1, 32_767, -32_768]);
    assert_eq!(run.samples, [0, 1, -1, 32_767, -32_768]);
    assert_eq!(run.observations, [DspObservation::PassedThrough]);
}

/// EFFECT-V4: a smoothed gain change is a linear ramp in positions, and it lands exactly on the
/// target — the transition is a function of position and not of how the caller cut the stream.
#[test]
fn gain_transitions_are_deterministic_in_positions() {
    let input = [1_000i16; 6];
    let expected = [1_250, 1_500, 1_750, 2_000, 2_000, 2_000];

    let mut whole = Gain::new();
    let run = once(
        &mut whole,
        &[ratio("gain", 2_000), integer("smoothing_positions", 4)],
        &input,
    );
    assert_eq!(run.samples, expected);
    assert_eq!(
        run.observations,
        [
            DspObservation::ParameterApplied {
                parameter: "gain",
                at_position: 0
            },
            DspObservation::ParameterApplied {
                parameter: "smoothing_positions",
                at_position: 0
            },
        ]
    );

    // The same ramp, cut differently.
    let mut split = Gain::new();
    let mut run = prepared(
        &mut split,
        &[ratio("gain", 2_000), integer("smoothing_positions", 4)],
    );
    run.feed(&mut split, d8(), &input[..2]).unwrap();
    run.feed(&mut split, d8(), &input[2..3]).unwrap();
    run.feed(&mut split, d8(), &input[3..]).unwrap();
    assert_eq!(run.samples, expected);
}

// ----------------------------------------------------------------------- polarity ----

/// EFFECT-V5: inverting `i16::MIN` clamps rather than wrapping, which is the one sample the
/// representable range is asymmetric about.
#[test]
fn polarity_inverts_without_wrapping() {
    let mut polarity = Polarity::new();
    let run = once(
        &mut polarity,
        &[flag("inverted", true)],
        &[0, 1, -1, 32_767, -32_768],
    );
    assert_eq!(run.samples, [0, -1, 1, -32_767, 32_767]);
    assert!(
        run.observations
            .contains(&DspObservation::Saturated { positions: 1 })
    );

    let mut straight = Polarity::new();
    let run = once(&mut straight, &[], &[0, 1, -1, 32_767, -32_768]);
    assert_eq!(run.samples, [0, 1, -1, 32_767, -32_768]);
    assert_eq!(run.observations, [DspObservation::PassedThrough]);
}

// ------------------------------------------------------------------------ clipping ----

/// EFFECT-V6: hard clipping holds a symmetric ceiling, and a ceiling below full scale is the
/// effect rather than a saturation — `Saturated` names a value clamped to the representable
/// maximum and nothing else.
#[test]
fn hard_clip_holds_its_ceiling() {
    let mut clip = HardClip::new();
    let run = once(
        &mut clip,
        &[integer("ceiling", 1_000)],
        &[0, 500, 1_000, 1_001, -1_000, -1_001, 32_767, -32_768],
    );
    assert_eq!(
        run.samples,
        [0, 500, 1_000, 1_000, -1_000, -1_000, 1_000, -1_000]
    );
    assert!(
        !run.observations
            .iter()
            .any(|observation| matches!(observation, DspObservation::Saturated { .. })),
        "{:?}",
        run.observations
    );
}

/// EFFECT-V7: soft clipping is a quadratic knee that never reaches full scale, so it never
/// reports `Saturated` at all.
#[test]
fn soft_clip_never_reaches_full_scale() {
    let mut clip = SoftClip::new();
    let run = once(
        &mut clip,
        &[integer("threshold", 16_384)],
        &[0, 16_384, 20_000, 32_767, -32_768],
    );
    assert_eq!(run.samples, [0, 16_384, 19_801, 28_672, -28_672]);
    assert!(
        !run.observations
            .iter()
            .any(|observation| matches!(observation, DspObservation::Saturated { .. })),
        "{:?}",
        run.observations
    );
}

// ----------------------------------------------------------------------- bit crush ----

/// EFFECT-V8: bit reduction is a floor quantisation onto the declared depth, and the hold is
/// counted in positions rather than in frames.
#[test]
fn bit_crush_quantises_and_holds() {
    let mut crush = BitCrush::new();
    let run = once(
        &mut crush,
        &[integer("bits", 8)],
        &[0, 255, 256, 32_767, -1, -32_768],
    );
    assert_eq!(run.samples, [0, 0, 256, 32_512, -256, -32_768]);

    let mut held = BitCrush::new();
    let run = once(
        &mut held,
        &[integer("hold_positions", 2)],
        &[100, 200, 300, 400],
    );
    assert_eq!(run.samples, [100, 100, 300, 300]);

    // The same hold, cut across the held pair.
    let mut split = BitCrush::new();
    let mut run = prepared(&mut split, &[integer("hold_positions", 2)]);
    run.feed(&mut split, d8(), &[100]).unwrap();
    run.feed(&mut split, d8(), &[200, 300]).unwrap();
    run.feed(&mut split, d8(), &[400]).unwrap();
    assert_eq!(run.samples, [100, 100, 300, 300]);
}

// ------------------------------------------------------------------------- stutter ----

/// EFFECT-V9: a bounded delay line writes one position out for one in, holds exactly its declared
/// tail, and hands that tail over on `flush` rather than losing it.
#[test]
fn stutter_delays_by_its_declared_line() {
    let mut stutter = Stutter::new(2).unwrap();
    assert_eq!(stutter.capability().latency_positions(), 2);
    assert_eq!(stutter.capability().tail_positions(), 2);

    let mut run = prepared(&mut stutter, &[]);
    run.feed(&mut stutter, d8(), &[1, 2, 3]).unwrap();
    assert_eq!(run.samples, [0, 0, 1]);
    assert_eq!(stutter.retained(), 2);
    run.feed(&mut stutter, d8(), &[4, 5, 6]).unwrap();
    assert_eq!(run.samples, [0, 0, 1, 2, 3, 4]);
    run.flush(&mut stutter, d8());
    assert_eq!(run.samples, [0, 0, 1, 2, 3, 4, 5, 6]);
    assert_eq!(stutter.retained(), 0);
}

/// EFFECT-V10: with `repeat` set the line stops taking new input and loops what it holds. That is
/// the whole of the glitch: a bounded, declared, repeatable transform of audio the processor
/// already had.
#[test]
fn stutter_repeats_a_bounded_line() {
    let mut stutter = Stutter::new(3).unwrap();
    let mut run = prepared(&mut stutter, &[]);
    run.feed(&mut stutter, d8(), &[1, 2, 3, 4, 5, 6]).unwrap();
    assert_eq!(run.samples, [0, 0, 0, 1, 2, 3]);

    stutter.configure(&[flag("repeat", true)]).unwrap();
    let mut looped = Run {
        position: run.position,
        ..Run::default()
    };
    looped.feed(&mut stutter, d8(), &[7, 8, 9]).unwrap();
    looped.feed(&mut stutter, d8(), &[10, 11, 12]).unwrap();
    assert_eq!(looped.samples, [4, 5, 6, 4, 5, 6]);
    assert_eq!(
        looped.observations.first(),
        Some(&DspObservation::ParameterApplied {
            parameter: "repeat",
            at_position: 6
        })
    );

    // Releasing the repeat resumes the delay from what the line still holds.
    stutter.configure(&[flag("repeat", false)]).unwrap();
    let mut resumed = Run {
        position: looped.position,
        ..Run::default()
    };
    resumed.feed(&mut stutter, d8(), &[13, 14, 15]).unwrap();
    assert_eq!(resumed.samples, [4, 5, 6]);
}

/// EFFECT-V11: zero and maximum delay are both admissible declarations and neither divides by
/// zero, allocates past its bound or refuses a frame.
#[test]
fn stutter_holds_at_zero_and_at_its_maximum() {
    let mut none = Stutter::new(0).unwrap();
    let run = once(&mut none, &[flag("repeat", true)], &[1, 2, 3]);
    assert_eq!(run.samples, [1, 2, 3]);
    assert_eq!(none.retained(), 0);

    let mut most = Stutter::new(MAX_STUTTER_POSITIONS).unwrap();
    assert_eq!(
        most.capability().tail_positions(),
        MAX_STUTTER_POSITIONS,
        "the declared tail is the line"
    );
    let run = once(&mut most, &[], &[1_000; 64]);
    assert_eq!(run.samples, [0i16; 64], "the line is still filling");

    assert!(Stutter::new(MAX_STUTTER_POSITIONS + 1).is_err());
}

/// Acceptance: an intentional glitch is distinguishable from an accidental discontinuity.
///
/// The two live in different vocabularies and neither can be mistaken for the other. A repeat is a
/// `ParameterApplied` — something the application asked for, at the position it took effect. A
/// break in the timeline is a `Restarted { cause: Discontinuity }` — something the seam declared
/// happened to the call. A deadline miss is neither: it is a runtime fact and `M-64` reports it as
/// a `Bypassed` transition, which no processor can emit.
#[test]
fn an_intentional_stutter_is_not_an_accidental_discontinuity() {
    let mut stutter = Stutter::new(2).unwrap();
    let mut run = prepared(&mut stutter, &[]);
    run.feed(&mut stutter, d8(), &[1, 2, 3, 4]).unwrap();
    stutter.configure(&[flag("repeat", true)]).unwrap();
    run.feed(&mut stutter, d8(), &[5, 6]).unwrap();

    let intentional: Vec<&DspObservation> = run
        .observations
        .iter()
        .filter(|observation| {
            matches!(observation, DspObservation::ParameterApplied { parameter, .. } if *parameter == "repeat")
        })
        .collect();
    assert_eq!(intentional.len(), 1);
    assert!(
        !run.observations
            .iter()
            .any(|observation| matches!(observation, DspObservation::Restarted { .. })),
        "a requested repeat is never reported as a restart: {:?}",
        run.observations
    );

    // The same processor, handed a break the seam declared, reports the restart and nothing that
    // could be read as a requested effect.
    let mut broken = Stutter::new(2).unwrap();
    let mut run = prepared(&mut broken, &[]);
    run.feed(&mut broken, d8(), &[1, 2, 3, 4]).unwrap();
    run.feed_at(
        &mut broken,
        d8(),
        &[5, 6],
        0,
        Some(DiscontinuityKind::Realign),
    )
    .unwrap();
    assert!(run.observations.contains(&DspObservation::Restarted {
        cause: DspResetCause::Discontinuity {
            kind: DiscontinuityKind::Realign
        }
    }));
    assert!(
        !run.observations
            .iter()
            .any(|observation| matches!(observation, DspObservation::ParameterApplied { .. })),
        "a declared break is never reported as a parameter the application applied"
    );
    // The break discarded the line, so the flagged frame opens the new epoch from silence.
    assert_eq!(run.samples, [0, 0, 1, 2, 0, 0]);
}

// ------------------------------------------------------------------------- filters ----

/// EFFECT-V12: at a cutoff of one quarter of the rate the shipped coefficient is exactly 0.5 and
/// the section is a two-tap average, so the impulse and step responses are exact.
#[test]
fn low_pass_impulse_and_step_are_exact() {
    let mut filter = LowPass::new();
    let run = once(
        &mut filter,
        &[integer("cutoff_hz", 2_000)],
        &[32_767, 0, 0, 0, 0],
    );
    assert_eq!(run.samples, [16_384, 16_384, 0, 0, 0]);

    let mut filter = LowPass::new();
    let run = once(&mut filter, &[integer("cutoff_hz", 2_000)], &[1_000; 4]);
    assert_eq!(run.samples, [500, 1_000, 1_000, 1_000]);
}

/// EFFECT-V13: the frequency vector. At that same cutoff the section has a zero at Nyquist and
/// unity gain at DC, exactly — so full-scale alternation settles to silence and a constant
/// settles to itself.
#[test]
fn low_pass_stops_nyquist_and_passes_dc() {
    let mut filter = LowPass::new();
    let run = once(
        &mut filter,
        &[integer("cutoff_hz", 2_000)],
        &[10_000, -10_000, 10_000, -10_000, 10_000],
    );
    assert_eq!(run.samples, [5_000, 0, 0, 0, 0]);

    let mut filter = LowPass::new();
    let run = once(&mut filter, &[integer("cutoff_hz", 2_000)], &[10_000; 5]);
    assert_eq!(run.samples, [5_000, 10_000, 10_000, 10_000, 10_000]);
}

/// EFFECT-V14: the high-pass is the low-pass's complement, position for position.
#[test]
fn high_pass_is_the_low_pass_complement() {
    let input = [32_767i16, 0, 0, 0, 0];
    let mut high = HighPass::new();
    let run = once(&mut high, &[integer("cutoff_hz", 2_000)], &input);
    assert_eq!(run.samples, [16_383, -16_384, 0, 0, 0]);

    let mut high = HighPass::new();
    let run = once(&mut high, &[integer("cutoff_hz", 2_000)], &[10_000; 5]);
    assert_eq!(run.samples, [5_000, 0, 0, 0, 0]);
}

/// EFFECT-V15: a cutoff above the folding frequency is clamped rather than refused. The parameter
/// domain is closed independently of the rate, and a rate-dependent refusal would make the same
/// configuration valid on one call and invalid on another. Clamped, the section is still a
/// low-pass: 512 positions of full-scale alternation leave it within one LSB of silence rather
/// than ringing up.
#[test]
fn a_cutoff_past_nyquist_is_clamped_and_stable() {
    let alternating: Vec<i16> = (0..512)
        .map(|index| if index % 2 == 0 { i16::MAX } else { i16::MIN })
        .collect();
    let mut filter = LowPass::new();
    let run = once(&mut filter, &[integer("cutoff_hz", 192_000)], &alternating);
    assert_eq!(run.samples.len(), alternating.len());
    let tail = run.samples.get(448..).unwrap_or(&[]);
    assert!(
        tail.iter().all(|sample| i32::from(*sample).abs() <= 1),
        "{tail:?}"
    );
}

/// EFFECT-V16: the peaking filter leaves DC and Nyquist where it found them — a band lift is a
/// band lift — and a unit band gain is the exact identity.
#[test]
fn peaking_leaves_dc_and_nyquist_alone() {
    // Both sections agree at DC, so the band is empty there whatever the gain is.
    let mut peaking = Peaking::new();
    let run = once(
        &mut peaking,
        &[integer("centre_hz", 1_000), ratio("band_gain", 4_000)],
        &[5_000; 512],
    );
    assert_eq!(run.samples.last(), Some(&5_000), "settled DC");

    // And at the folding frequency, for the same reason.
    let mut peaking = Peaking::new();
    let alternating: Vec<i16> = (0..512)
        .map(|index| if index % 2 == 0 { 10_000 } else { -10_000 })
        .collect();
    let run = once(
        &mut peaking,
        &[integer("centre_hz", 1_000), ratio("band_gain", 4_000)],
        &alternating,
    );
    assert_eq!(
        run.samples.get(508..),
        Some([10_000i16, -10_000, 10_000, -10_000].as_slice())
    );

    // A unit band gain is the exact identity, which is what makes the default harmless.
    let mut peaking = Peaking::new();
    let run = once(&mut peaking, &[], &[7, -7, 7, -7, 7, -7, 7, -7]);
    assert_eq!(run.samples, [7, -7, 7, -7, 7, -7, 7, -7]);
    assert_eq!(
        run.observations.last(),
        Some(&DspObservation::PassedThrough)
    );
}

/// EFFECT-V17: boosting and cutting the same band are exact complements about the input, which is
/// the arithmetic statement of "the band is extracted once and scaled".
#[test]
fn peaking_boost_and_cut_are_complementary() {
    let input = [10_000i16, 0, 0, 0, 0, 0, 0, 0];
    let mut boost = Peaking::new();
    let boosted = once(
        &mut boost,
        &[integer("centre_hz", 1_000), ratio("band_gain", 2_000)],
        &input,
    );
    assert_eq!(
        boosted.samples,
        [13_341, 2_232, -1_849, -1_236, -826, -552, -369, -246]
    );

    let mut cut = Peaking::new();
    let cutted = once(
        &mut cut,
        &[integer("centre_hz", 1_000), ratio("band_gain", 0)],
        &input,
    );
    assert_eq!(
        cutted.samples,
        [6_659, -2_232, 1_849, 1_236, 826, 552, 369, 246]
    );

    for ((boosted, cut), input) in boosted.samples.iter().zip(&cutted.samples).zip(&input) {
        assert_eq!(i32::from(*boosted) + i32::from(*cut), 2 * i32::from(*input));
    }
}

// ------------------------------------------------------------------------ extremes ----

/// Acceptance: extreme amplitude and arbitrary finite PCM never produce a wraparound, a panic or
/// an out-of-range sample, whatever the parameters are.
#[test]
fn hostile_input_never_wraps_or_panics() {
    let hostile: Vec<Vec<i16>> = vec![
        vec![i16::MAX; 32],
        vec![i16::MIN; 32],
        (0..32)
            .map(|index| if index % 2 == 0 { i16::MAX } else { i16::MIN })
            .collect(),
        vec![0; 32],
        vec![-1; 32],
        (0..32)
            .map(|index: i32| i16::try_from((index * 4_099) % 19_997 - 9_998).unwrap_or(0))
            .collect(),
    ];

    for input in &hostile {
        for parameters in [
            vec![ratio("gain", 8_000)],
            vec![ratio("gain", 0)],
            vec![integer("smoothing_positions", 4_096), ratio("gain", 8_000)],
        ] {
            let mut gain = Gain::new();
            let run = once(&mut gain, &parameters, input);
            assert_eq!(run.samples.len(), input.len());
        }

        let mut polarity = Polarity::new();
        assert_eq!(
            once(&mut polarity, &[flag("inverted", true)], input)
                .samples
                .len(),
            input.len()
        );

        for ceiling in [0i64, 1, 32_767] {
            let mut clip = HardClip::new();
            let run = once(&mut clip, &[integer("ceiling", ceiling)], input);
            assert!(
                run.samples
                    .iter()
                    .all(|sample| i64::from(sample.abs()) <= ceiling),
                "ceiling {ceiling}: {:?}",
                run.samples
            );
        }

        for threshold in [0, 1, 16_384, 32_766] {
            let mut clip = SoftClip::new();
            let run = once(&mut clip, &[integer("threshold", threshold)], input);
            assert_eq!(run.samples.len(), input.len());
        }

        for bits in [1, 8, 16] {
            for hold in [1, 3, 256] {
                let mut crush = BitCrush::new();
                let run = once(
                    &mut crush,
                    &[integer("bits", bits), integer("hold_positions", hold)],
                    input,
                );
                assert_eq!(run.samples.len(), input.len());
            }
        }

        for cutoff in [1, 300, 3_400, 192_000] {
            let mut low = LowPass::new();
            assert_eq!(
                once(&mut low, &[integer("cutoff_hz", cutoff)], input)
                    .samples
                    .len(),
                input.len()
            );
            let mut high = HighPass::new();
            assert_eq!(
                once(&mut high, &[integer("cutoff_hz", cutoff)], input)
                    .samples
                    .len(),
                input.len()
            );
            let mut peaking = Peaking::new();
            assert_eq!(
                once(
                    &mut peaking,
                    &[integer("centre_hz", cutoff), ratio("band_gain", 4_000)],
                    input
                )
                .samples
                .len(),
                input.len()
            );
        }

        for delay in [0, 1, 7, 64] {
            let mut stutter = Stutter::new(delay).unwrap();
            assert_eq!(
                once(&mut stutter, &[flag("repeat", true)], input)
                    .samples
                    .len(),
                input.len()
            );
        }
    }
}

/// Acceptance: an invalid coefficient, rate or parameter refuses the whole set and leaves the
/// previous one in force, and a rate outside the declaration refuses at `prepare`.
#[test]
fn invalid_parameters_and_rates_refuse_without_partial_state() {
    let mut gain = Gain::new();
    gain.configure(&[ratio("gain", 2_000)]).unwrap();

    for offered in [
        ratio("gain", 8_001),
        ratio("gain", -1),
        integer("gain", 2),
        ratio("unknown", 0),
        integer("smoothing_positions", 4_097),
    ] {
        assert!(
            gain.configure(&[offered]).is_err(),
            "{offered:?} was accepted"
        );
    }
    // A set is refused entire: a valid parameter beside an invalid one changes nothing.
    assert!(
        gain.configure(&[ratio("gain", 1_000), ratio("gain", 9_999)])
            .is_err()
    );

    let run = once(&mut gain, &[], &[1_000, 1_000]);
    assert_eq!(
        run.samples,
        [2_000, 2_000],
        "the 2.0 gain is still in force"
    );

    // Nine channels are outside the contract altogether, and three are outside this declaration.
    assert!(StreamFormat::new(8_000, 9).is_err());
    let mut filter = LowPass::new();
    assert!(
        filter
            .prepare(
                AudioDirection::Inbound,
                StreamFormat::new(8_000, 3).unwrap()
            )
            .is_err()
    );
    assert!(filter.prepare(AudioDirection::Inbound, d8()).is_ok());
}

/// EFFECT-V18: chunk-boundary independence, over every built-in at once.
///
/// One stream, cut at 7, 1, 13, 5 and 22 positions, against the same stream in one frame. Every
/// duration inside these processors is counted in positions rather than in frames, which is what
/// makes this hold — and the flushed tail is compared too, so a delay line cannot pass by holding
/// something different.
fn cuts<P: FrameProcessor>(processor: &mut P, parameters: &[Parameter], input: &[i16]) -> Run {
    let mut run = prepared(processor, parameters);
    let mut offset = 0;
    for step in [7usize, 1, 13, 5, 22].iter().cycle() {
        if offset >= input.len() {
            break;
        }
        let end = (offset + step).min(input.len());
        run.feed(processor, d8(), &input[offset..end]).unwrap();
        offset = end;
    }
    run.flush(processor, d8());
    run
}

#[test]
fn chunk_boundaries_never_change_a_stream() {
    let input: Vec<i16> = (0..48)
        .map(|index: i32| i16::try_from((index * 2_617) % 19_997 - 9_998).unwrap_or(0))
        .collect();

    macro_rules! agree {
        ($build:expr, $parameters:expr) => {{
            let mut whole = $build;
            let mut split = $build;
            let mut reference = prepared(&mut whole, $parameters);
            reference.feed(&mut whole, d8(), &input).unwrap();
            reference.flush(&mut whole, d8());
            let divided = cuts(&mut split, $parameters, &input);
            assert_eq!(reference.samples, divided.samples);
        }};
    }

    agree!(Gain::new(), &[ratio("gain", 3_000)]);
    agree!(Polarity::new(), &[flag("inverted", true)]);
    agree!(HardClip::new(), &[integer("ceiling", 4_000)]);
    agree!(SoftClip::new(), &[integer("threshold", 2_000)]);
    agree!(
        BitCrush::new(),
        &[integer("bits", 5), integer("hold_positions", 3)]
    );
    agree!(Stutter::new(5).unwrap(), &[]);
    agree!(LowPass::new(), &[integer("cutoff_hz", 900)]);
    agree!(HighPass::new(), &[integer("cutoff_hz", 900)]);
    agree!(
        Peaking::new(),
        &[integer("centre_hz", 700), ratio("band_gain", 2_500)]
    );
}

/// Acceptance: a reset discards sample memory rather than flushing it, exactly as §8.1 requires of
/// every processor — and a stateful built-in says so through its declaration.
#[test]
fn a_reset_discards_rather_than_flushing() {
    for capability in [
        LowPass::new().capability(),
        HighPass::new().capability(),
        Peaking::new().capability(),
        BitCrush::new().capability(),
        Stutter::new(4).unwrap().capability(),
    ] {
        assert_eq!(
            capability.reset(),
            ResetBehavior::ClearsState,
            "{}",
            capability.id()
        );
    }
    // A gain holds no sample memory at all: its ramp is parameter state, and §8.1 keeps declared
    // parameters across a reset. A reset of it is therefore unobservable, which is what
    // `Stateless` declares.
    for capability in [
        Gain::new().capability(),
        Polarity::new().capability(),
        HardClip::new().capability(),
        SoftClip::new().capability(),
    ] {
        assert_eq!(
            capability.reset(),
            ResetBehavior::Stateless,
            "{}",
            capability.id()
        );
    }

    let mut stutter = Stutter::new(2).unwrap();
    let mut run = prepared(&mut stutter, &[]);
    run.feed(&mut stutter, d8(), &[1, 2, 3, 4]).unwrap();
    assert_eq!(stutter.retained(), 2);
    stutter.reset(DspResetCause::Requested);
    assert_eq!(stutter.retained(), 0);

    let mut after = Run::default();
    after.feed(&mut stutter, d8(), &[9, 9]).unwrap();
    assert_eq!(after.samples, [0, 0], "the held audio was discarded");
}

/// Acceptance: cancellation is terminal and releases the line, for the one built-in that owns a
/// line at all.
#[test]
fn cancellation_releases_a_delay_line() {
    let mut stutter = Stutter::new(64).unwrap();
    let mut run = prepared(&mut stutter, &[]);
    run.feed(&mut stutter, d8(), &[1_000; 32]).unwrap();
    assert_eq!(stutter.retained(), 32);

    stutter.cancel();
    assert_eq!(stutter.retained(), 0);
    stutter.cancel();
    assert_eq!(stutter.retained(), 0);

    let mut refused = Run::default();
    assert_eq!(
        refused.feed(&mut stutter, d8(), &[1, 2, 3]),
        Err(ProcessError::Cancelled)
    );
}

/// Stereo is interleaved, channel 0 first, and per-channel state never crosses channels.
#[test]
fn stereo_channels_never_interleave_state() {
    let stereo = StreamFormat::new(8_000, 2).unwrap();
    let mut filter = LowPass::new();
    filter.configure(&[integer("cutoff_hz", 2_000)]).unwrap();
    filter.prepare(AudioDirection::Inbound, stereo).unwrap();

    let mut run = Run::default();
    // Left carries the impulse; right carries silence and must stay silent.
    run.feed(&mut filter, stereo, &[32_767, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    assert_eq!(run.samples, [16_384, 0, 16_384, 0, 0, 0, 0, 0]);
}
