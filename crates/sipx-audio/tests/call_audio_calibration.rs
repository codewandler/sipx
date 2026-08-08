//! The `CAL-*` vectors of [`docs/specs/call-audio-processing.md`](../../../docs/specs/call-audio-processing.md) §12.
//!
//! `M-60` layers bounded threshold calibration on `M-57`'s contract. The contract's promise does not
//! change because a threshold moves: identical inputs still produce identical observations on every
//! machine, so the *evolution* of the threshold is itself a fixture rather than a statistical
//! argument. Each test below names the vector it replays and the section that defines the
//! expectation, and every number in it is copied from the specification rather than read off a run.
//!
//! The corpus runs against the crate's public API only — an integration test, so a vector that needs
//! something the published surface cannot express is a finding about the surface.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_audio::analysis::{
    AnalysisError, AnalysisFrame, AnalysisProfile, AudioAnalyzer, AudioDirection,
    CalibrationOutcome, CalibrationProfile, DiscontinuityKind, Observation, ProfileError,
    ResetCause, VoiceEndCause, window_deviation,
};

// ---------------------------------------------------------------------------------------------
// §12.12's reference profiles and the patterns the vectors are written in
// ---------------------------------------------------------------------------------------------

/// §11.1's reference profile `P8`: inbound, 8,000 Hz, 20 ms windows (`W = 160`).
fn p8() -> AnalysisProfile {
    AnalysisProfile::new(AudioDirection::Inbound, 8_000)
}

/// §12.12's reference calibration profile `K8`.
///
/// `calibration_ms = 200` (C = 1,600), `update_ms = 100` (U = 800), `margin_amplitude = 512`,
/// `floor_amplitude = 256`, `ceiling_amplitude = 8,192`, `max_step_amplitude = 128`. Periods span
/// five windows and end at epoch samples 800, 1,600, 2,400, …
fn k8() -> CalibrationProfile {
    CalibrationProfile::new()
}

/// `P8` with `K8` in force.
fn calibrated() -> AnalysisProfile {
    p8().with_calibration(Some(k8()))
}

/// One window of alternating `±amplitude`.
///
/// Its deviation (§12.2) is exactly `amplitude`: 80 samples of each sign give `sum = 0` and
/// `energy = W·amplitude²`, so `V = W²·amplitude²` and `isqrt(V) div W = amplitude`.
fn alternating(amplitude: i16) -> Vec<i16> {
    (0..160)
        .map(|index| {
            if index % 2 == 0 {
                amplitude
            } else {
                -amplitude
            }
        })
        .collect()
}

fn silence() -> Vec<i16> {
    vec![0i16; 160]
}

/// CAP-W4's pattern: one full-scale sample in an otherwise empty window.
fn impulse() -> Vec<i16> {
    let mut samples = vec![0i16; 160];
    samples[40] = 32_767;
    samples
}

/// Feed every frame in order, draining after each one as §11.1 requires.
fn run(profile: AnalysisProfile, frames: &[Vec<i16>]) -> Vec<Observation> {
    let mut analyzer = AudioAnalyzer::new(profile).unwrap();
    let mut seen = Vec::new();
    for (index, samples) in frames.iter().enumerate() {
        let sequence = u64::try_from(index).unwrap();
        analyzer
            .process(&AnalysisFrame::new(profile.direction(), sequence, samples))
            .unwrap();
        seen.extend(analyzer.drain());
    }
    seen
}

/// Every threshold update in a drain sequence, as `(at_sample, new, previous, observed_floor)`.
fn updates(observations: &[Observation]) -> Vec<(u64, i32, i32, i32)> {
    observations
        .iter()
        .filter_map(|observation| match observation {
            Observation::ThresholdUpdated {
                at_sample,
                activation_amplitude,
                previous,
                observed_floor,
            } => Some((
                *at_sample,
                *activation_amplitude,
                *previous,
                *observed_floor,
            )),
            _ => None,
        })
        .collect()
}

fn repeated(samples: &[i16], frames: usize) -> Vec<Vec<i16>> {
    vec![samples.to_vec(); frames]
}

fn field_of(error: &AnalysisError) -> &'static str {
    match error {
        AnalysisError::Profile(ProfileError::Field { field, .. }) => field,
        other => panic!("expected a refused field, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// §12.12 Threshold evolution
// ---------------------------------------------------------------------------------------------

/// CAL-1 — long silence walks the threshold down to its floor in declared steps, and stops there.
#[test]
fn cal_1_long_silence_walks_the_threshold_to_its_floor_and_stops() {
    let observations = run(calibrated(), &repeated(&silence(), 70));

    // The period ending at 800 is inside the calibration period, so nothing moves until 1,600.
    let expected: Vec<(u64, i32, i32, i32)> = (0..12)
        .map(|step| {
            (
                1_600 + 800 * step,
                1_920 - 128 * i32::try_from(step).unwrap(),
                2_048 - 128 * i32::try_from(step).unwrap(),
                0,
            )
        })
        .collect();
    assert_eq!(updates(&observations), expected);

    assert!(
        !observations
            .iter()
            .any(|observation| matches!(observation, Observation::VoiceStarted { .. })),
        "silence is never voice at any threshold in the declared interval"
    );
}

/// CAL-1's end state, read through the inspection surface rather than the drain.
#[test]
fn cal_1_the_threshold_settles_one_margin_above_the_observed_floor() {
    let mut analyzer = AudioAnalyzer::new(calibrated()).unwrap();
    for sequence in 0..70u64 {
        analyzer
            .process(&AnalysisFrame::new(
                AudioDirection::Inbound,
                sequence,
                &silence(),
            ))
            .unwrap();
        let _ = analyzer.drain();
    }

    let thresholds = analyzer.thresholds();
    assert_eq!(thresholds.activation_amplitude(), 512, "0 + margin 512");
    assert_eq!(thresholds.observed_floor(), Some(0));
    assert_eq!(thresholds.outcome(), Some(CalibrationOutcome::Applied));
    assert_eq!(thresholds.updates(), 12);
    assert_eq!(thresholds.calibration_samples(), Some(1_600));
    assert_eq!(thresholds.update_samples(), Some(800));
}

/// CAL-2 — a noise ramp is tracked at exactly one margin, and the ceiling holds.
#[test]
fn cal_2_a_noise_ramp_is_tracked_at_one_margin_and_stops_at_the_ceiling() {
    // `A_0 = 512`, so the threshold rises with the noise instead of first falling to meet it.
    let profile = p8()
        .with_activation_amplitude(512)
        .with_calibration(Some(k8()));
    // Deviation `64·p` held constant across the five frames of period `p`.
    let frames: Vec<Vec<i16>> = (0..625)
        .map(|index: i32| alternating(i16::try_from(64 * (index / 5)).unwrap()))
        .collect();

    let observations = run(profile, &frames);
    let seen = updates(&observations);

    // Period `p` ends at 800·(p+1) and leaves the threshold at `64·p + 512`: the ramp is followed
    // exactly one margin above, by a 64-sample step that never reaches the 128 limit.
    let expected: Vec<(u64, i32, i32, i32)> = (1..=120i32)
        .map(|period| {
            (
                800 * (u64::try_from(period).unwrap() + 1),
                512 + 64 * period,
                512 + 64 * (period - 1),
                64 * period,
            )
        })
        .collect();
    assert_eq!(seen, expected);
    assert!(
        seen.iter().all(|(_, next, _, _)| *next <= 8_192),
        "the ceiling is never crossed"
    );
    assert!(
        !observations
            .iter()
            .any(|observation| matches!(observation, Observation::VoiceStarted { .. })),
        "a ramp the threshold keeps ahead of is never called voice"
    );
}

/// CAL-3 — a sudden step is crossed in declared steps, and the freeze limit bounds the interval it
/// would otherwise hold open forever.
#[test]
fn cal_3_a_sudden_step_is_crossed_in_declared_steps_and_bounds_the_active_interval() {
    let profile = p8().with_activation_amplitude(512).with_calibration(Some(
        k8().with_max_step_amplitude(1_024)
            .with_freeze_limit_ms(Some(200)),
    ));
    let mut frames = repeated(&silence(), 10);
    frames.extend(repeated(&alternating(4_000), 60));

    let observations = run(profile, &frames);

    assert_eq!(
        updates(&observations),
        vec![
            (3_200, 1_536, 512, 4_000),
            (4_800, 2_560, 1_536, 4_000),
            (6_400, 3_584, 2_560, 4_000),
            (8_000, 4_512, 3_584, 4_000),
        ],
        "the freeze limit forces one bounded step every 1,600 samples until the step is cleared"
    );

    let voice: Vec<&Observation> = observations
        .iter()
        .filter(|observation| {
            matches!(
                observation,
                Observation::VoiceStarted { .. } | Observation::VoiceEnded { .. }
            )
        })
        .collect();
    assert_eq!(
        voice,
        vec![
            &Observation::VoiceStarted { at_sample: 1_600 },
            &Observation::VoiceEnded {
                at_sample: 8_000,
                cause: VoiceEndCause::Hangover,
            },
        ],
        "the interval a step opened closes: it is not unbounded"
    );
}

/// CAL-4 — input that sits exactly on the threshold settles to one value and stays there.
#[test]
fn cal_4_alternating_near_threshold_input_settles_deterministically() {
    let profile = p8().with_calibration(Some(k8().with_freeze_limit_ms(Some(200))));
    let frames: Vec<Vec<i16>> = (0..50)
        .map(|index| alternating(if index % 2 == 0 { 2_048 } else { 2_047 }))
        .collect();

    let observations = run(profile, &frames);

    assert_eq!(
        updates(&observations),
        vec![
            (2_400, 2_176, 2_048, 2_047),
            (4_000, 2_304, 2_176, 2_047),
            (4_800, 2_432, 2_304, 2_047),
            (5_600, 2_559, 2_432, 2_047),
        ],
        "the threshold settles one margin above the quietest eligible window and stops"
    );
    let voice: Vec<&Observation> = observations
        .iter()
        .filter(|observation| {
            matches!(
                observation,
                Observation::VoiceStarted { .. } | Observation::VoiceEnded { .. }
            )
        })
        .collect();
    assert_eq!(
        voice,
        vec![
            &Observation::VoiceStarted { at_sample: 0 },
            &Observation::VoiceEnded {
                at_sample: 2_400,
                cause: VoiceEndCause::Hangover,
            },
        ],
        "flapping across the threshold does not flap the events"
    );
}

// ---------------------------------------------------------------------------------------------
// §12.6 Freeze conditions, §12.11 what calibration cannot do
// ---------------------------------------------------------------------------------------------

/// CAL-5a — clipping is not a measurement of background noise, and moves nothing.
#[test]
fn cal_5a_clipping_measures_nothing_and_moves_nothing() {
    let mut analyzer = AudioAnalyzer::new(calibrated()).unwrap();
    let stuck = vec![32_767i16; 160];
    for sequence in 0..40u64 {
        analyzer
            .process(&AnalysisFrame::new(
                AudioDirection::Inbound,
                sequence,
                &stuck,
            ))
            .unwrap();
        assert!(
            !analyzer
                .drain()
                .any(|observation| matches!(observation, Observation::ThresholdUpdated { .. })),
            "a clipped window is ineligible, so there is nothing to update from"
        );
    }

    let thresholds = analyzer.thresholds();
    assert_eq!(thresholds.activation_amplitude(), 2_048, "unmoved");
    assert_eq!(
        thresholds.outcome(),
        Some(CalibrationOutcome::NoMeasurement)
    );
    assert_eq!(thresholds.observed_floor(), None, "nothing was measured");
    assert!(!analyzer.is_voiced(), "a constant signal has variance 0");
}

/// CAL-5b — a DC bias is not a measurement of background noise either.
#[test]
fn cal_5b_a_dc_bias_measures_nothing_and_moves_nothing() {
    let mut analyzer = AudioAnalyzer::new(calibrated()).unwrap();
    let biased = vec![1_000i16; 160];
    for sequence in 0..40u64 {
        analyzer
            .process(&AnalysisFrame::new(
                AudioDirection::Inbound,
                sequence,
                &biased,
            ))
            .unwrap();
        let _ = analyzer.drain();
    }

    let thresholds = analyzer.thresholds();
    assert_eq!(thresholds.activation_amplitude(), 2_048);
    assert_eq!(
        thresholds.outcome(),
        Some(CalibrationOutcome::NoMeasurement)
    );
    assert!(!analyzer.is_voiced());
}

/// CAL-5c — a single impulse contributes nothing, so the evolution is the one silence alone gives.
#[test]
fn cal_5c_a_single_impulse_changes_no_threshold() {
    let quiet = run(calibrated(), &repeated(&silence(), 30));

    let mut frames = repeated(&silence(), 30);
    frames[12] = impulse();
    let clicked = run(calibrated(), &frames);

    assert_eq!(
        updates(&clicked),
        updates(&quiet),
        "an impulsive window is excluded before it can define the quiet level"
    );
}

// ---------------------------------------------------------------------------------------------
// §12.8 Reset semantics
// ---------------------------------------------------------------------------------------------

/// CAL-6 — a requested reset discards the period and re-arms the warm-up, and keeps the threshold.
#[test]
fn cal_6_a_reset_clears_the_measurement_and_keeps_the_threshold() {
    let mut analyzer = AudioAnalyzer::new(calibrated()).unwrap();
    for sequence in 0..25u64 {
        analyzer
            .process(&AnalysisFrame::new(
                AudioDirection::Inbound,
                sequence,
                &silence(),
            ))
            .unwrap();
        let _ = analyzer.drain();
    }
    let before = analyzer.thresholds();
    assert_eq!(before.activation_amplitude(), 1_536);
    assert_eq!(before.updates(), 4);

    analyzer.reset();
    let after = analyzer.thresholds();
    assert_eq!(
        after.activation_amplitude(),
        1_536,
        "the timeline broke; the room did not"
    );
    assert_eq!(after.updates(), 4, "the update count is preserved");
    assert_eq!(after.observed_floor(), None, "the period is discarded");
    assert_eq!(after.outcome(), None, "the warm-up is re-armed");

    // No update may apply until C samples of the *new* epoch have completed windows.
    let mut seen = Vec::new();
    for sequence in 0..15u64 {
        analyzer
            .process(&AnalysisFrame::new(
                AudioDirection::Inbound,
                sequence,
                &silence(),
            ))
            .unwrap();
        seen.extend(analyzer.drain());
    }
    assert_eq!(
        updates(&seen).first(),
        Some(&(1_600, 1_408, 1_536, 0)),
        "the first update of the new epoch is at its own sample 1,600"
    );
}

/// CAL-6b — a format change re-derives the calibration counts against the new rate.
#[test]
fn cal_6b_a_format_change_re_derives_the_calibration_counts() {
    let mut analyzer = AudioAnalyzer::new(calibrated()).unwrap();
    for sequence in 0..25u64 {
        analyzer
            .process(&AnalysisFrame::new(
                AudioDirection::Inbound,
                sequence,
                &silence(),
            ))
            .unwrap();
        let _ = analyzer.drain();
    }

    analyzer.declare_format(16_000).unwrap();
    let thresholds = analyzer.thresholds();

    assert_eq!(thresholds.calibration_samples(), Some(3_200));
    assert_eq!(thresholds.update_samples(), Some(1_600));
    assert_eq!(thresholds.freeze_limit_samples(), Some(480_000));
    assert_eq!(
        thresholds.activation_amplitude(),
        1_536,
        "an amplitude is not a rate, so it survives the re-derivation"
    );
    assert!(analyzer.drain().any(|observation| matches!(
        observation,
        Observation::Reset {
            cause: ResetCause::FormatChange { rate: 16_000 }
        }
    )),);
}

/// CAL-6c — a flagged discontinuity re-arms the warm-up exactly as a requested reset does.
#[test]
fn cal_6c_a_flagged_discontinuity_re_arms_the_warm_up() {
    let mut analyzer = AudioAnalyzer::new(calibrated()).unwrap();
    for sequence in 0..25u64 {
        analyzer
            .process(&AnalysisFrame::new(
                AudioDirection::Inbound,
                sequence,
                &silence(),
            ))
            .unwrap();
        let _ = analyzer.drain();
    }

    analyzer
        .process(
            &AnalysisFrame::new(AudioDirection::Inbound, 25, &silence())
                .with_discontinuity(DiscontinuityKind::Loss),
        )
        .unwrap();
    let _ = analyzer.drain();

    let thresholds = analyzer.thresholds();
    assert_eq!(thresholds.activation_amplitude(), 1_536);
    assert_eq!(thresholds.outcome(), None, "the warm-up is re-armed");
}

// ---------------------------------------------------------------------------------------------
// §12.9 Inspection, §12.10 bounds, §12.11 limits
// ---------------------------------------------------------------------------------------------

/// CAL-7 — two directions calibrate independently and neither can read the other's audio.
#[test]
fn cal_7_two_directions_calibrate_independently() {
    let inbound_profile = p8().with_calibration(Some(k8()));
    let outbound_profile =
        AnalysisProfile::new(AudioDirection::Outbound, 8_000).with_calibration(Some(k8()));
    let mut inbound = AudioAnalyzer::new(inbound_profile).unwrap();
    let mut outbound = AudioAnalyzer::new(outbound_profile).unwrap();

    let noise = alternating(1_000);
    for sequence in 0..30u64 {
        inbound
            .process(&AnalysisFrame::new(
                AudioDirection::Inbound,
                sequence,
                &silence(),
            ))
            .unwrap();
        outbound
            .process(&AnalysisFrame::new(
                AudioDirection::Outbound,
                sequence,
                &noise,
            ))
            .unwrap();
        let _ = inbound.drain();
        let _ = outbound.drain();
    }

    // Five updates each, from one starting point, to two different places: the silent side is
    // walking toward `0 + margin` and the noisy side has already reached `1,000 + margin`.
    assert_eq!(inbound.thresholds().activation_amplitude(), 1_408);
    assert_eq!(inbound.thresholds().observed_floor(), Some(0));
    assert_eq!(outbound.thresholds().activation_amplitude(), 1_512);
    assert_eq!(outbound.thresholds().observed_floor(), Some(1_000));

    // Neither analyser can be fed the other's audio at all (§7.3), so there is no path by which
    // one call's floor could reach the other's threshold.
    assert!(
        inbound
            .process(&AnalysisFrame::new(AudioDirection::Outbound, 30, &noise))
            .is_err()
    );
}

/// CAL-8 — identical input produces identical threshold evolution and identical events, twice.
#[test]
fn cal_8_identical_input_produces_identical_evolution_across_runs() {
    let step = {
        let mut frames = repeated(&silence(), 10);
        frames.extend(repeated(&alternating(4_000), 40));
        frames
    };
    let near: Vec<Vec<i16>> = (0..50)
        .map(|index| alternating(if index % 2 == 0 { 2_048 } else { 2_047 }))
        .collect();
    let ramp: Vec<Vec<i16>> = (0..80)
        .map(|index: i32| alternating(i16::try_from(64 * (index / 5)).unwrap()))
        .collect();

    let profile = p8().with_calibration(Some(k8().with_freeze_limit_ms(Some(200))));
    for frames in [&repeated(&silence(), 70), &step, &near, &ramp] {
        assert_eq!(
            run(profile, frames),
            run(profile, frames),
            "two analysers from one profile fed one input drain the same sequence"
        );
    }
}

/// CAL-9 — calibration is never more sensitive than its declared floor.
///
/// The strong form of §12.11's first clause: every window adaptation makes `active` is a window the
/// *fixed* profile at `floor_amplitude` also makes active, so the set of intervals adaptation can
/// open is a subset of one that does not adapt at all.
#[test]
fn cal_9_adaptation_is_never_more_sensitive_than_its_floor() {
    let mut frames = repeated(&silence(), 20);
    frames.extend(repeated(&vec![32_767i16; 160], 5));
    frames.extend(repeated(&vec![1_000i16; 160], 5));
    frames.extend(repeated(&impulse(), 5));
    frames.extend((0..40).map(|index: i32| alternating(i16::try_from(100 * index).unwrap())));
    frames.extend(repeated(&silence(), 30));

    let adapting = run(calibrated(), &frames);
    let fixed = run(p8().with_activation_amplitude(256), &frames);

    let active_of = |observations: &[Observation]| -> Vec<bool> {
        observations
            .iter()
            .filter_map(|observation| match observation {
                Observation::Window { active, .. } => Some(*active),
                _ => None,
            })
            .collect()
    };
    let adapting_active = active_of(&adapting);
    let fixed_active = active_of(&fixed);
    assert_eq!(adapting_active.len(), fixed_active.len());
    for (index, (adapted, floored)) in adapting_active.iter().zip(&fixed_active).enumerate() {
        assert!(
            !*adapted || *floored,
            "window {index} is active under adaptation but not at the declared floor"
        );
    }
}

/// CAL-10 — extreme samples at the largest declared window neither overflow nor leave the interval.
#[test]
fn cal_10_extreme_samples_do_not_overflow_or_leave_the_declared_interval() {
    // The largest window §5.1 admits at the highest supported rate: 65,280 samples.
    let calibration = k8()
        .with_calibration_ms(1)
        .with_update_ms(1)
        .with_margin_amplitude(32_767)
        .with_floor_amplitude(1)
        .with_ceiling_amplitude(32_767)
        .with_max_step_amplitude(32_767)
        .with_freeze_limit_ms(Some(1));
    let profile = p8().with_window_ms(170).with_calibration(Some(calibration));
    let mut analyzer = AudioAnalyzer::new(profile).unwrap();
    analyzer.declare_format(384_000).unwrap();
    let _ = analyzer.drain();
    assert_eq!(analyzer.window_samples(), 65_280);

    let full_scale: Vec<i16> = (0..65_280)
        .map(|index| if index % 2 == 0 { i16::MIN } else { i16::MAX })
        .collect();
    analyzer
        .process(&AnalysisFrame::new(AudioDirection::Inbound, 0, &full_scale))
        .unwrap();
    let observations: Vec<Observation> = analyzer.drain().collect();
    let Observation::Window { sum, energy, .. } = observations[0] else {
        panic!("expected a window first, got {observations:?}");
    };
    assert!(
        (0..=32_768).contains(&window_deviation(65_280, sum, energy)),
        "the deviation of any supported sample fits the amplitude domain"
    );
    assert_eq!(
        analyzer.thresholds().activation_amplitude(),
        2_048,
        "a clipped window is ineligible, so nothing moved"
    );

    let loud: Vec<i16> = (0..65_280)
        .map(|index| if index % 2 == 0 { 32_766 } else { -32_766 })
        .collect();
    analyzer
        .process(&AnalysisFrame::new(AudioDirection::Inbound, 1, &loud))
        .unwrap();
    let _ = analyzer.drain();
    assert_eq!(
        analyzer.thresholds().activation_amplitude(),
        32_767,
        "the target saturates at the ceiling rather than overflowing past it"
    );
}

/// CAL-11 — inspection reads, and only reads.
#[test]
fn cal_11_inspection_does_not_mutate_and_carries_no_audio() {
    let profile = calibrated();
    let mut watched = AudioAnalyzer::new(profile).unwrap();
    let mut unwatched = AudioAnalyzer::new(profile).unwrap();
    let mut watched_seen = Vec::new();
    let mut unwatched_seen = Vec::new();
    let mut early = None;

    let quiet = silence();
    for sequence in 0..40u64 {
        let frame = AnalysisFrame::new(AudioDirection::Inbound, sequence, &quiet);
        watched.process(&frame).unwrap();
        unwatched.process(&frame).unwrap();
        for _ in 0..4 {
            let snapshot = watched.thresholds();
            if sequence == 12 && early.is_none() {
                early = Some(snapshot);
            }
        }
        watched_seen.extend(watched.drain());
        unwatched_seen.extend(unwatched.drain());
    }

    assert_eq!(
        watched_seen, unwatched_seen,
        "reading the thresholds changes nothing an observer can see"
    );
    // A snapshot is a value, not a view: what it said at sample 2,080 it still says.
    let early = early.unwrap();
    assert_eq!(early.activation_amplitude(), 1_920);
    assert_ne!(
        early.activation_amplitude(),
        watched.thresholds().activation_amplitude()
    );
    // Everything it carries is a count or an amplitude; the profile it names is the configured one.
    assert_eq!(early.profile(), profile);
    assert_eq!(early.window_samples(), 160);
    assert_eq!(early.hangover_samples(), 1_600);
    assert_eq!(early.silence_timeout_samples(), Some(16_000));
    assert_eq!(early.calibration(), Some(k8()));
}

/// CAL-12 — an analyser with no calibration profile behaves exactly as §5 and §6 already specify.
#[test]
fn cal_12_an_analyser_without_calibration_never_adapts() {
    let frames: Vec<Vec<i16>> = (0..60)
        .map(|index: i32| alternating(i16::try_from(50 * index).unwrap()))
        .collect();
    let observations = run(p8(), &frames);

    assert!(updates(&observations).is_empty());
    let mut analyzer = AudioAnalyzer::new(p8()).unwrap();
    analyzer
        .process(&AnalysisFrame::new(
            AudioDirection::Inbound,
            0,
            &alternating(1_000),
        ))
        .unwrap();
    let thresholds = analyzer.thresholds();
    assert_eq!(thresholds.calibration(), None);
    assert_eq!(thresholds.calibration_samples(), None);
    assert_eq!(thresholds.update_samples(), None);
    assert_eq!(thresholds.outcome(), None);
    assert_eq!(thresholds.activation_amplitude(), 2_048);
}

// ---------------------------------------------------------------------------------------------
// §12.3 Refused configurations
// ---------------------------------------------------------------------------------------------

/// CAL-C1 — every calibration domain is checked, and the refusal names the field.
#[test]
fn cal_c1_every_calibration_domain_is_refused_by_name() {
    let cases: Vec<(CalibrationProfile, &'static str)> = vec![
        (k8().with_calibration_ms(0), "calibration_ms"),
        (k8().with_update_ms(0), "update_ms"),
        (k8().with_margin_amplitude(0), "margin_amplitude"),
        (k8().with_margin_amplitude(32_768), "margin_amplitude"),
        (k8().with_floor_amplitude(0), "floor_amplitude"),
        (k8().with_ceiling_amplitude(32_768), "ceiling_amplitude"),
        (k8().with_max_step_amplitude(0), "max_step_amplitude"),
        (k8().with_freeze_limit_ms(Some(0)), "freeze_limit_ms"),
    ];
    for (calibration, field) in cases {
        let error = AudioAnalyzer::new(p8().with_calibration(Some(calibration)))
            .expect_err("the domain is refused");
        assert_eq!(field_of(&error), field, "{calibration:?}");
    }
}

/// CAL-C2 — a ceiling below its floor is not an interval, and is refused rather than swapped.
#[test]
fn cal_c2_a_ceiling_below_its_floor_is_refused() {
    let error = AudioAnalyzer::new(p8().with_calibration(Some(
        k8().with_floor_amplitude(1_000).with_ceiling_amplitude(999),
    )))
    .expect_err("an empty interval is refused");
    assert_eq!(field_of(&error), "ceiling_amplitude");
}

/// CAL-C3 — the configured activation amplitude must lie in the interval calibration may move it
/// through, so the invariant `floor <= A <= ceiling` holds from sample 0 without silent clamping.
#[test]
fn cal_c3_an_activation_amplitude_outside_the_interval_is_refused() {
    let error = AudioAnalyzer::new(
        p8().with_activation_amplitude(100)
            .with_calibration(Some(k8())),
    )
    .expect_err("100 is below the 256 floor");
    assert_eq!(field_of(&error), "activation_amplitude");

    let error = AudioAnalyzer::new(
        p8().with_activation_amplitude(9_000)
            .with_calibration(Some(k8())),
    )
    .expect_err("9,000 is above the 8,192 ceiling");
    assert_eq!(field_of(&error), "activation_amplitude");
}

/// CAL-C4 — a derived calibration count that does not fit `u32` is refused naming its field.
#[test]
fn cal_c4_a_derived_count_that_does_not_fit_is_refused() {
    let error = AudioAnalyzer::new(p8().with_calibration(Some(k8().with_calibration_ms(u32::MAX))))
        .expect_err("u32::MAX ms at 8,000 Hz derives more than u32::MAX samples");
    match error {
        AnalysisError::Profile(ProfileError::DerivedCount { field, .. }) => {
            assert_eq!(field, "calibration_ms");
        }
        other => panic!("expected a derived-count refusal, got {other:?}"),
    }
}
