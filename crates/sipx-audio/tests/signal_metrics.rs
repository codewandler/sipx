//! `docs/specs/call-audio-processing.md` §11's `CAP-*` vectors, for the signal-metric half of the
//! contract (`M-59`).
//!
//! These are the story's failing-first vectors: they pin the exact energy and level units, the
//! window boundaries, the clipping definition and the silence transition before anything computes
//! them. Every expected number below is transcribed from the specification's §11 tables, and the
//! ones the spec does not state (the derived level) are stated here as exact integer arithmetic
//! rather than as an approximation with a tolerance.
//!
//! What is deliberately *not* here: `VoiceStarted`/`VoiceEnded` and the hangover machine of §6.
//! That is `M-58`'s slice of the same contract. The per-window `active` fact is computed and
//! counted because §5.3 defines it as one of the five window facts and the vectors below pin it,
//! but no activity *transition* is derived from it in this crate.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_audio::signal::{
    FrameError, ProfileError, ResetCause, SignalDirection, SignalDiscontinuity, SignalFrame,
    SignalObservation, SignalProcessor, SignalProfile, SignalReport,
};
use sipx_audio::{Pcm, PcmEncoding, PcmError, PcmFormat, PcmSamples};

/// §11.1's reference profile `P8`, minus the hangover `M-58` owns.
///
/// One window per report, so a report is exactly one window's facts and the §11.2 vectors read
/// directly off it.
fn p8() -> SignalProfile {
    SignalProfile::new(SignalDirection::Inbound, 8_000)
        .with_window_ms(20)
        .with_activation_amplitude(2_048)
        .with_silence_amplitude(64)
        .with_impulse_amplitude(16_384)
        .with_dc_amplitude(512)
        .with_clip_samples(8)
        .with_silence_timeout_ms(Some(2_000))
        .with_windows_per_report(1)
        .with_queue_capacity(64)
}

fn processor() -> SignalProcessor {
    SignalProcessor::new(p8()).expect("the reference profile is inside every domain")
}

/// Feed one in-sequence frame and drain, as §11.1's caller does.
fn feed(processor: &mut SignalProcessor, sequence: u64, samples: &[i16]) -> Vec<SignalObservation> {
    processor
        .process(SignalFrame::new(SignalDirection::Inbound, sequence, samples))
        .expect("an in-sequence inbound frame is accepted");
    processor.drain().collect()
}

fn reports(observations: &[SignalObservation]) -> Vec<SignalReport> {
    observations
        .iter()
        .filter_map(|observation| match observation {
            SignalObservation::Report(report) => Some(*report),
            _ => None,
        })
        .collect()
}

/// The one report a single window produces.
fn one_report(observations: &[SignalObservation]) -> SignalReport {
    let found = reports(observations);
    assert_eq!(found.len(), 1, "expected one report: {observations:?}");
    found[0]
}

fn alternating() -> Vec<i16> {
    (0..160)
        .map(|index| if index % 2 == 0 { 8_192 } else { -8_192 })
        .collect()
}

fn impulse() -> Vec<i16> {
    let mut samples = vec![0i16; 160];
    samples[40] = 32_767;
    samples
}

// ---------------------------------------------------------------------------------------------
// §11.2 Window facts. The level and energy units, and the clipping definition.
// ---------------------------------------------------------------------------------------------

/// CAP-W1 — silence measures zero and is the only fact the window carries.
#[test]
fn cap_w1_a_silent_window_reports_zero_level_and_nothing_else() {
    let mut processor = processor();
    let report = one_report(&feed(&mut processor, 0, &[0i16; 160]));

    assert_eq!(report.peak, 0);
    assert_eq!(report.sum, 0);
    assert_eq!(report.energy, 0);
    assert_eq!(report.rms, 0);
    assert_eq!(report.clipped_samples, 0);
    assert_eq!(report.silent_windows, 1);
    assert_eq!(report.clipping_windows, 0);
    assert_eq!(report.impulsive_windows, 0);
    assert_eq!(report.active_windows, 0);
    assert_eq!(report.dc_offset_windows, 0);
}

/// CAP-W2 — modulation at half scale: the exact energy, and the variance predicate that makes it
/// signal rather than a stuck level.
#[test]
fn cap_w2_alternating_half_scale_reports_the_specified_energy_and_level() {
    let mut processor = processor();
    let report = one_report(&feed(&mut processor, 0, &alternating()));

    assert_eq!(report.peak, 8_192);
    assert_eq!(report.sum, 0);
    assert_eq!(report.energy, 10_737_418_240);
    // floor(sqrt(floor(energy / samples))) = floor(sqrt(67_108_864)) = 8192, in sample amplitude.
    assert_eq!(report.rms, 8_192);
    assert_eq!(report.clipped_samples, 0);
    assert_eq!(report.active_windows, 1);
    assert_eq!(report.silent_windows, 0);
    assert_eq!(report.clipping_windows, 0);
    assert_eq!(report.dc_offset_windows, 0);
}

/// CAP-W3 — a stuck full-scale DAC clips and offsets, and is deliberately *not* active: its
/// variance is exactly zero.
#[test]
fn cap_w3_a_stuck_full_scale_window_clips_and_offsets_without_being_active() {
    let mut processor = processor();
    let report = one_report(&feed(&mut processor, 0, &[32_767i16; 160]));

    assert_eq!(report.peak, 32_767);
    assert_eq!(report.sum, 5_242_720);
    assert_eq!(report.energy, 171_788_206_240);
    assert_eq!(report.rms, 32_767);
    assert_eq!(report.clipped_samples, 160);
    assert_eq!(report.clipping_windows, 1);
    assert_eq!(report.dc_offset_windows, 1);
    assert_eq!(report.active_windows, 0, "W·energy − sum² is exactly zero");
    assert_eq!(report.silent_windows, 0);
}

/// CAP-W4 — one full-scale click is impulsive, and is none of the other four facts.
#[test]
fn cap_w4_a_single_full_scale_click_is_impulsive_and_nothing_else() {
    let mut processor = processor();
    let report = one_report(&feed(&mut processor, 0, &impulse()));

    assert_eq!(report.peak, 32_767);
    assert_eq!(report.sum, 32_767);
    assert_eq!(report.energy, 1_073_676_289);
    // floor(sqrt(floor(1_073_676_289 / 160))) = floor(sqrt(6_710_476)) = 2590.
    assert_eq!(report.rms, 2_590);
    assert_eq!(report.clipped_samples, 1);
    assert_eq!(report.impulsive_windows, 1);
    assert_eq!(report.active_windows, 0);
    assert_eq!(report.clipping_windows, 0, "one clipped sample is below 8");
    assert_eq!(report.dc_offset_windows, 0);
    assert_eq!(report.silent_windows, 0);
}

/// CAP-W5 — a constant offset: level and DC, but no modulation and no silence.
#[test]
fn cap_w5_a_constant_offset_reports_dc_without_activity_or_silence() {
    let mut processor = processor();
    let report = one_report(&feed(&mut processor, 0, &[1_000i16; 160]));

    assert_eq!(report.peak, 1_000);
    assert_eq!(report.sum, 160_000);
    assert_eq!(report.energy, 160_000_000);
    assert_eq!(report.rms, 1_000);
    assert_eq!(report.clipped_samples, 0);
    assert_eq!(report.dc_offset_windows, 1);
    assert_eq!(report.active_windows, 0);
    assert_eq!(report.silent_windows, 0);
}

/// The negative full-scale sample is clipping too, and its magnitude is 32,768 (§5.2).
#[test]
fn negative_full_scale_clips_and_peaks_at_the_magnitude_the_spec_states() {
    let mut processor = processor();
    let report = one_report(&feed(&mut processor, 0, &[i16::MIN; 160]));

    assert_eq!(report.peak, 32_768, "|−32768| = 32768");
    assert_eq!(report.clipped_samples, 160);
    assert_eq!(report.clipping_windows, 1);
}

// ---------------------------------------------------------------------------------------------
// Window boundaries and coverage.
// ---------------------------------------------------------------------------------------------

/// A window is exactly `W` samples aligned to the stream position; a frame shorter than a window
/// completes nothing and the next frame finishes it.
#[test]
fn windows_are_exactly_w_samples_aligned_to_the_stream_and_span_frame_boundaries() {
    let mut processor = processor();

    assert!(
        reports(&feed(&mut processor, 0, &[1_000i16; 100])).is_empty(),
        "100 samples do not complete a 160-sample window"
    );

    let observations = feed(&mut processor, 1, &[1_000i16; 220]);
    let found = reports(&observations);
    assert_eq!(found.len(), 2, "320 samples in total complete two windows");
    assert_eq!(found[0].first_window, 0);
    assert_eq!(found[0].at_sample, 0);
    assert_eq!(found[0].samples, 160);
    assert_eq!(found[1].first_window, 1);
    assert_eq!(found[1].at_sample, 160);
    assert_eq!(found[0].sum, 160_000, "the window spans the frame boundary");
    assert_eq!(found[1].sum, 160_000);
}

/// A report names the windows it covers, so coverage is readable rather than inferred from a
/// cadence the consumer has to know.
#[test]
fn a_report_names_its_window_and_sample_coverage() {
    let mut processor = SignalProcessor::new(p8().with_windows_per_report(4)).unwrap();

    let observations = feed(&mut processor, 0, &[1_000i16; 640]);
    let report = one_report(&observations);

    assert_eq!(report.rate, 8_000);
    assert_eq!(report.sequence, 0);
    assert_eq!(report.first_window, 0);
    assert_eq!(report.windows, 4);
    assert_eq!(report.at_sample, 0);
    assert_eq!(report.samples, 640);
    assert_eq!(report.sum, 640_000);
    assert_eq!(report.energy, 640_000_000);
    assert_eq!(report.rms, 1_000);
    assert_eq!(report.dc_offset_windows, 4);
}

/// The cadence bounds how many events a call produces: four windows per report is a quarter of
/// the reports, and a partial period at the end of the input is not reported early.
#[test]
fn the_reporting_cadence_coalesces_windows_and_never_reports_a_partial_period() {
    let mut processor = SignalProcessor::new(p8().with_windows_per_report(4)).unwrap();

    let observations = feed(&mut processor, 0, &[1_000i16; 160 * 7]);
    let found = reports(&observations);

    assert_eq!(found.len(), 1, "seven windows are one full period and a half");
    assert_eq!(found[0].windows, 4);

    let observations = feed(&mut processor, 1, &[1_000i16; 160]);
    let found = reports(&observations);
    assert_eq!(found.len(), 1, "the eighth window completes the period");
    assert_eq!(found[0].first_window, 4);
    assert_eq!(found[0].sequence, 1);
}

// ---------------------------------------------------------------------------------------------
// §11.3 The silence transition.
// ---------------------------------------------------------------------------------------------

/// CAP-A2 — the silence timeout fires once when the run first reaches the derived count, and
/// re-arms only after a non-silent window.
#[test]
fn cap_a2_the_silence_timeout_fires_once_at_the_derived_sample_count() {
    let mut processor = processor();
    let mut elapsed = Vec::new();

    for sequence in 0..100 {
        for observation in feed(&mut processor, sequence, &[0i16; 160]) {
            if let SignalObservation::SilenceElapsed { at_sample, epoch } = observation {
                elapsed.push((at_sample, epoch));
            }
        }
    }

    assert_eq!(
        elapsed,
        vec![(0, 0)],
        "16,000 samples of silence at 8,000 Hz is the 2,000 ms timeout, and it names the run's \
         first sample"
    );

    for observation in feed(&mut processor, 100, &[0i16; 160]) {
        assert!(
            !matches!(observation, SignalObservation::SilenceElapsed { .. }),
            "a still-silent window does not re-fire the timeout"
        );
    }
}

/// The run clears on a non-silent window and the timeout can fire again — the transition back out
/// of silence is what re-arms it.
#[test]
fn a_non_silent_window_clears_the_silence_run_and_re_arms_the_timeout() {
    let mut processor = processor();

    for sequence in 0..100 {
        feed(&mut processor, sequence, &[0i16; 160]);
    }
    feed(&mut processor, 100, &[1_000i16; 160]);

    let mut elapsed = Vec::new();
    for sequence in 101..201 {
        for observation in feed(&mut processor, sequence, &[0i16; 160]) {
            if let SignalObservation::SilenceElapsed { at_sample, .. } = observation {
                elapsed.push(at_sample);
            }
        }
    }

    assert_eq!(
        elapsed,
        vec![160 * 101],
        "the second run starts at the first silent sample after the non-silent window"
    );
}

/// `silence_timeout_ms: None` disables the timer entirely (§6).
#[test]
fn no_silence_timeout_means_no_silence_observation_however_long_the_quiet_lasts() {
    let mut processor = SignalProcessor::new(p8().with_silence_timeout_ms(None)).unwrap();

    for sequence in 0..200 {
        for observation in feed(&mut processor, sequence, &[0i16; 160]) {
            assert!(
                !matches!(observation, SignalObservation::SilenceElapsed { .. }),
                "the timer is off"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// §11.4 Format changes, and §11.5 sequence and discontinuity.
// ---------------------------------------------------------------------------------------------

/// CAP-F1 — a format change resets under its own cause and re-derives the window length.
#[test]
fn cap_f1_a_format_change_resets_and_re_derives_the_window() {
    let mut processor = processor();
    feed(&mut processor, 0, &[0i16; 160]);

    processor.declare_format(16_000).unwrap();
    let observations: Vec<_> = processor.drain().collect();
    assert!(
        matches!(
            observations.as_slice(),
            [SignalObservation::Reset {
                cause: ResetCause::FormatChange { rate: 16_000 },
                epoch: 1,
            }]
        ),
        "expected exactly one format-change reset: {observations:?}"
    );

    let report = one_report(&feed(&mut processor, 1, &[0i16; 320]));
    assert_eq!(report.rate, 16_000);
    assert_eq!(report.epoch, 1);
    assert_eq!(report.first_window, 0, "the new epoch starts at window 0");
    assert_eq!(report.samples, 320, "W = ceil(20 · 16000 / 1000)");
}

/// CAP-F2 — a refused format change leaves the previous format in force and emits nothing.
#[test]
fn cap_f2_a_refused_format_change_does_not_half_apply() {
    let mut processor = processor();

    assert!(matches!(
        processor.declare_format(0),
        Err(ProfileError::UnsupportedSampleRate(
            PcmError::UnsupportedSampleRate(0)
        ))
    ));
    assert!(matches!(
        processor.declare_format(384_001),
        Err(ProfileError::UnsupportedSampleRate(
            PcmError::UnsupportedSampleRate(384_001)
        ))
    ));
    assert_eq!(processor.drain().count(), 0, "a refusal observes nothing");

    let report = one_report(&feed(&mut processor, 0, &[0i16; 160]));
    assert_eq!(report.rate, 8_000, "8,000 Hz is still in force");
    assert_eq!(report.epoch, 0, "no epoch was opened");
}

/// CAP-F3 — the derivation rounds up, so a window never covers less than its declared duration.
#[test]
fn cap_f3_the_window_derivation_rounds_up() {
    let mut processor = processor();
    processor.declare_format(8_193).unwrap();
    let _ = processor.drain().count();

    let report = one_report(&feed(&mut processor, 0, &[0i16; 164]));
    assert_eq!(report.samples, 164, "ceil(20 · 8193 / 1000) = 164");
}

/// CAP-S1 — a flagged gap discards the partial window unemitted, resets, and opens the new epoch
/// with the flagged frame's own samples.
#[test]
fn cap_s1_a_flagged_gap_discards_the_partial_window_and_opens_a_new_epoch() {
    let mut processor = processor();
    assert!(reports(&feed(&mut processor, 0, &[1_000i16; 100])).is_empty());

    processor
        .process(
            SignalFrame::new(SignalDirection::Inbound, 5, &[0i16; 160])
                .with_discontinuity(SignalDiscontinuity::Loss),
        )
        .unwrap();
    let observations: Vec<_> = processor.drain().collect();

    assert!(
        matches!(
            observations.first(),
            Some(SignalObservation::Reset {
                cause: ResetCause::Discontinuity {
                    kind: SignalDiscontinuity::Loss
                },
                epoch: 1,
            })
        ),
        "the reset precedes the flagged frame's own samples: {observations:?}"
    );
    let report = one_report(&observations);
    assert_eq!(report.epoch, 1);
    assert_eq!(report.first_window, 0);
    assert_eq!(report.at_sample, 0);
    assert_eq!(
        report.sum, 0,
        "the 100 discarded samples are not in the new epoch's window"
    );

    processor
        .process(SignalFrame::new(SignalDirection::Inbound, 6, &[0i16; 160]))
        .expect("the sequence base was re-established at 5");
}

/// CAP-S2 and CAP-S3 — an unflagged gap and a repeat are refused, and a refusal changes nothing.
#[test]
fn cap_s2_and_s3_a_gap_or_a_repeat_is_refused_without_touching_the_stream() {
    let mut processor = processor();
    feed(&mut processor, 0, &[1_000i16; 160]);

    assert!(matches!(
        processor.process(SignalFrame::new(SignalDirection::Inbound, 2, &[0i16; 160])),
        Err(FrameError::MalformedSequence { .. })
    ));
    assert!(matches!(
        processor.process(SignalFrame::new(SignalDirection::Inbound, 0, &[0i16; 160])),
        Err(FrameError::MalformedSequence { .. })
    ));
    assert!(matches!(
        processor.process(
            SignalFrame::new(SignalDirection::Inbound, 0, &[0i16; 160])
                .with_discontinuity(SignalDiscontinuity::Realign)
        ),
        Err(FrameError::MalformedSequence { .. })
    ));
    assert_eq!(processor.drain().count(), 0);

    let report = one_report(&feed(&mut processor, 1, &[1_000i16; 160]));
    assert_eq!(report.first_window, 1, "the position never moved");
    assert_eq!(report.epoch, 0, "and no epoch was opened");
}

/// CAP-S4 — a flagged frame need not skip a sequence number.
#[test]
fn cap_s4_a_flagged_frame_at_the_next_sequence_still_resets() {
    let mut processor = processor();
    feed(&mut processor, 0, &[0i16; 160]);

    processor
        .process(
            SignalFrame::new(SignalDirection::Inbound, 1, &[0i16; 160])
                .with_discontinuity(SignalDiscontinuity::Overflow),
        )
        .unwrap();
    let observations: Vec<_> = processor.drain().collect();

    assert!(matches!(
        observations.first(),
        Some(SignalObservation::Reset {
            cause: ResetCause::Discontinuity {
                kind: SignalDiscontinuity::Overflow
            },
            ..
        })
    ));
}

/// A requested reset discards the partial period and opens an epoch of its own (§7.1).
#[test]
fn a_requested_reset_discards_the_partial_period_and_opens_an_epoch() {
    let mut processor = SignalProcessor::new(p8().with_windows_per_report(4)).unwrap();
    feed(&mut processor, 0, &[1_000i16; 480]);

    processor.reset();
    let observations: Vec<_> = processor.drain().collect();
    assert!(
        matches!(
            observations.as_slice(),
            [SignalObservation::Reset {
                cause: ResetCause::Requested,
                epoch: 1,
            }]
        ),
        "three completed windows were a partial period and are not reported: {observations:?}"
    );

    let report = one_report(&feed(&mut processor, 0, &[0i16; 640]));
    assert_eq!(report.epoch, 1);
    assert_eq!(report.sequence, 0, "report sequence restarts with the epoch");
    assert_eq!(report.at_sample, 0);
}

// ---------------------------------------------------------------------------------------------
// §11.6 the queue bound, §11.7 refused frames, §11.8 refused configurations.
// ---------------------------------------------------------------------------------------------

/// CAP-Q1 — an undersized queue coalesces into a counted, deterministic marker rather than
/// blocking or growing.
#[test]
fn cap_q1_an_overrun_queue_coalesces_into_a_counted_marker() {
    let mut processor = SignalProcessor::new(p8().with_queue_capacity(2)).unwrap();
    processor
        .process(SignalFrame::new(SignalDirection::Inbound, 0, &[0i16; 480]))
        .unwrap();

    let observations: Vec<_> = processor.drain().collect();
    assert_eq!(observations.len(), 2);
    assert!(matches!(
        observations[0],
        SignalObservation::Report(SignalReport {
            first_window: 0,
            ..
        })
    ));
    assert!(matches!(
        observations[1],
        SignalObservation::Lost { count: 2 }
    ));
}

/// CAP-N1, CAP-N2, CAP-N3 — the three frame refusals, each leaving the stream untouched.
#[test]
fn cap_n1_n2_n3_refused_frames_change_nothing() {
    let mut processor = processor();

    assert!(matches!(
        processor.process(SignalFrame::new(SignalDirection::Inbound, 0, &[])),
        Err(FrameError::MalformedFrame { samples: 0 })
    ));
    assert!(matches!(
        processor.process(SignalFrame::new(
            SignalDirection::Inbound,
            0,
            &vec![0i16; 65_537]
        )),
        Err(FrameError::MalformedFrame { samples: 65_537 })
    ));
    assert!(matches!(
        processor.process(SignalFrame::new(
            SignalDirection::Outbound,
            0,
            &[0i16; 160]
        )),
        Err(FrameError::DirectionMismatch { .. })
    ));
    assert_eq!(processor.drain().count(), 0);

    let report = one_report(&feed(&mut processor, 0, &[0i16; 160]));
    assert_eq!(report.first_window, 0, "sequence and position are untouched");
}

/// CAP-C1 through CAP-C5 — every refused configuration, named by its field, before allocation.
#[test]
fn cap_c1_through_c5_refuse_every_out_of_domain_configuration() {
    assert!(matches!(
        SignalProcessor::new(p8().with_window_ms(0)),
        Err(ProfileError::WindowMs { .. })
    ));
    assert!(
        matches!(
            SignalProcessor::new(
                SignalProfile::new(SignalDirection::Inbound, 384_000).with_window_ms(200)
            ),
            Err(ProfileError::WindowMs { samples: 76_800, .. })
        ),
        "a derived 76,800-sample window is refused, never clamped"
    );
    assert!(matches!(
        SignalProcessor::new(p8().with_queue_capacity(1)),
        Err(ProfileError::QueueCapacity { requested: 1 })
    ));
    assert!(matches!(
        SignalProcessor::new(p8().with_queue_capacity(4_097)),
        Err(ProfileError::QueueCapacity { requested: 4_097 })
    ));
    assert!(matches!(
        SignalProcessor::new(SignalProfile::new(SignalDirection::Inbound, 0)),
        Err(ProfileError::UnsupportedSampleRate(
            PcmError::UnsupportedSampleRate(0)
        ))
    ));
    assert!(matches!(
        SignalProcessor::new(SignalProfile::new(SignalDirection::Inbound, 384_001)),
        Err(ProfileError::UnsupportedSampleRate(
            PcmError::UnsupportedSampleRate(384_001)
        ))
    ));
    assert!(matches!(
        SignalProcessor::new(p8().with_silence_timeout_ms(Some(0))),
        Err(ProfileError::SilenceTimeoutMs { milliseconds: 0 })
    ));
    assert!(matches!(
        SignalProcessor::new(p8().with_clip_samples(0)),
        Err(ProfileError::ClipSamples { requested: 0, .. })
    ));
    assert!(
        matches!(
            SignalProcessor::new(p8().with_clip_samples(161)),
            Err(ProfileError::ClipSamples {
                requested: 161,
                window: 160
            })
        ),
        "clip_samples is bounded by the derived window"
    );
    assert!(matches!(
        SignalProcessor::new(p8().with_windows_per_report(0)),
        Err(ProfileError::WindowsPerReport { requested: 0 })
    ));
    assert!(matches!(
        SignalProcessor::new(p8().with_activation_amplitude(0)),
        Err(ProfileError::ActivationAmplitude { value: 0 })
    ));
    assert!(matches!(
        SignalProcessor::new(p8().with_silence_amplitude(32_769)),
        Err(ProfileError::SilenceAmplitude { value: 32_769 })
    ));
}

/// A re-derivation that leaves the domain is refused and the previous format survives (§7.2).
#[test]
fn a_format_change_whose_window_leaves_the_domain_is_refused() {
    let mut processor = SignalProcessor::new(p8().with_window_ms(200)).unwrap();

    assert!(matches!(
        processor.declare_format(384_000),
        Err(ProfileError::WindowMs { samples: 76_800, .. })
    ));
    assert_eq!(processor.drain().count(), 0);

    let report = one_report(&feed(&mut processor, 0, &[0i16; 1_600]));
    assert_eq!(report.rate, 8_000, "the previous format is untouched");
}

// ---------------------------------------------------------------------------------------------
// Determinism, and every supported rate and PCM format.
// ---------------------------------------------------------------------------------------------

/// CAP-D1 — two processors from one profile, fed one input, drain identically.
#[test]
fn cap_d1_two_processors_produce_identical_drain_sequences() {
    let frames: Vec<Vec<i16>> = std::iter::once(alternating())
        .chain((0..11).map(|_| vec![0i16; 160]))
        .collect();

    let drained = |profile: SignalProfile| {
        let mut processor = SignalProcessor::new(profile).unwrap();
        let mut observed = Vec::new();
        for (sequence, samples) in frames.iter().enumerate() {
            processor
                .process(SignalFrame::new(
                    SignalDirection::Inbound,
                    sequence as u64,
                    samples,
                ))
                .unwrap();
            observed.extend(processor.drain());
        }
        observed
    };

    assert_eq!(drained(p8()), drained(p8()));
}

/// Every rate in the `linear-pcm.md` domain derives its own window, including both ends of it.
#[test]
fn every_supported_rate_derives_its_own_window_length() {
    for (rate, window) in [
        (1u32, 1u64),
        (8_000, 160),
        (16_000, 320),
        (44_100, 882),
        (48_000, 960),
        (384_000, 7_680),
    ] {
        let mut processor = SignalProcessor::new(
            SignalProfile::new(SignalDirection::Inbound, rate)
                .with_window_ms(20)
                .with_clip_samples(1),
        )
        .unwrap_or_else(|error| panic!("rate {rate} must be accepted: {error}"));

        let samples = vec![1_000i16; usize::try_from(window).unwrap()];
        let report = one_report(&feed(&mut processor, 0, &samples));
        assert_eq!(report.rate, rate);
        assert_eq!(report.samples, window, "ceil(20 · {rate} / 1000)");
        assert_eq!(report.peak, 1_000);
    }
}

/// Both `linear-pcm.md` representations reach the same numbers, because the boundary converts
/// once and the processor consumes only the signed form.
#[test]
fn both_supported_pcm_representations_produce_the_declared_numbers() {
    let signed = vec![1_000i16; 160];
    // 1000 is not representable in unsigned 8-bit, whose step is 256; the boundary truncates to
    // the step below and the metrics describe what actually arrived, not what was asked for.
    let unsigned = Pcm::from_i16(
        PcmFormat::new(8_000, PcmEncoding::Unsigned8).unwrap(),
        signed.clone(),
    );
    assert!(matches!(unsigned.samples(), PcmSamples::Unsigned8(_)));
    let converted = unsigned.to_i16(8_000).unwrap();

    let mut signed_processor = processor();
    let from_signed = one_report(&feed(&mut signed_processor, 0, &signed));

    let mut unsigned_processor = processor();
    let from_unsigned = one_report(&feed(&mut unsigned_processor, 0, &converted));

    assert_eq!(from_signed.peak, 1_000);
    assert_eq!(from_unsigned.peak, 768, "the 8-bit step below 1000");
    assert_eq!(from_unsigned.dc_offset_windows, 1);
    assert_eq!(from_unsigned.rms, 768);
}

/// The processor is signal content and says so: nothing it reports is loss, jitter, round trip or
/// MOS, and no accessor of `M-10`'s network-quality surface appears in this vocabulary.
#[test]
fn the_vocabulary_names_signal_content_and_not_network_quality() {
    let names = format!("{:?}", one_report(&feed(&mut processor(), 0, &[0i16; 160])));
    for network in ["loss", "jitter", "round_trip", "mos"] {
        assert!(
            !names.contains(network),
            "a signal report must not borrow M-10's {network} vocabulary: {names}"
        );
    }
}
