//! Signal metrics over `docs/specs/call-audio-processing.md` §11's `CAP-*` sample vectors
//! (`M-59`).
//!
//! These are the story's failing-first vectors: they pin the exact energy and level units, the
//! window boundaries, the clipping definition and the silence transition **as an application
//! receives them** — through [`SignalReport`], not through the analyser's internal window facts,
//! which `call_audio_analysis.rs` already pins for `M-58`.
//!
//! The distinction matters. `M-59` adds no arithmetic over samples: every expected number below
//! is either one of the analyser's own accumulators carried through unchanged, or the exactly
//! stated integer derivation of `rms`. What is genuinely new here, and therefore what most of
//! this file tests, is coverage: which windows a report covers, what happens to a period a reset
//! or a lost observation holes, and that a report can never describe an epoch or a rate other
//! than its own.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_audio::analysis::{
    AnalysisFrame, AnalysisProfile, AudioAnalyzer, AudioDirection, DiscontinuityKind, ResetCause,
};
use sipx_audio::signal::{SignalObservation, SignalReport, SignalReportProfile, SignalReporter};
use sipx_audio::{Pcm, PcmEncoding, PcmFormat, PcmSamples};

/// §11.1's reference profile `P8`, with the silence timer its table declares.
fn p8() -> AnalysisProfile {
    AnalysisProfile::new(AudioDirection::Inbound, 8_000)
        .with_window_ms(20)
        .with_activation_amplitude(2_048)
        .with_silence_amplitude(64)
        .with_impulse_amplitude(16_384)
        .with_dc_amplitude(512)
        .with_clip_samples(8)
        .with_hangover_ms(200)
        .with_silence_timeout_ms(Some(2_000))
        .with_queue_capacity(64)
}

/// One analyser and the reporter summarising it, as the call layer pairs them.
struct Metered {
    analyzer: AudioAnalyzer,
    reporter: SignalReporter,
}

impl Metered {
    fn new(profile: SignalReportProfile) -> Self {
        Self {
            analyzer: AudioAnalyzer::new(profile.analysis()).expect("a valid analysis profile"),
            reporter: SignalReporter::new(profile).expect("a valid reporting profile"),
        }
    }

    /// Feed one in-sequence frame and collect the signal observations it produced.
    fn feed(&mut self, sequence: u64, samples: &[i16]) -> Vec<SignalObservation> {
        self.analyzer
            .process(&AnalysisFrame::new(
                AudioDirection::Inbound,
                sequence,
                samples,
            ))
            .expect("an in-sequence inbound frame is accepted");
        self.collect()
    }

    fn flagged(
        &mut self,
        sequence: u64,
        kind: DiscontinuityKind,
        samples: &[i16],
    ) -> Vec<SignalObservation> {
        self.analyzer
            .process(
                &AnalysisFrame::new(AudioDirection::Inbound, sequence, samples)
                    .with_discontinuity(kind),
            )
            .expect("a flagged frame is accepted");
        self.collect()
    }

    fn declare_format(&mut self, rate: u32) -> Vec<SignalObservation> {
        self.analyzer.declare_format(rate).expect("a valid rate");
        self.collect()
    }

    fn collect(&mut self) -> Vec<SignalObservation> {
        let window = self.analyzer.window_samples();
        let reporter = &mut self.reporter;
        self.analyzer
            .drain()
            .filter_map(|observation| reporter.observe(&observation, window))
            .collect()
    }
}

fn metered(windows_per_report: u32) -> Metered {
    Metered::new(SignalReportProfile::new(p8()).with_windows_per_report(windows_per_report))
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
// §11.2 window facts, as an application receives them: the level and energy units, and the
// clipping definition.
// ---------------------------------------------------------------------------------------------

/// CAP-W1 — silence measures zero and is the only fact the window carries.
#[test]
fn cap_w1_a_silent_window_reports_zero_level_and_nothing_else() {
    let report = one_report(&metered(1).feed(0, &[0i16; 160]));

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

/// CAP-W2 — modulation at half scale: the exact energy the specification names, and the level
/// derived from it.
#[test]
fn cap_w2_alternating_half_scale_reports_the_specified_energy_and_level() {
    let report = one_report(&metered(1).feed(0, &alternating()));

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

/// CAP-W3 — a stuck full-scale capture clips and offsets, and is deliberately not active: its
/// variance is exactly zero.
#[test]
fn cap_w3_a_stuck_full_scale_window_clips_and_offsets_without_being_active() {
    let report = one_report(&metered(1).feed(0, &[32_767i16; 160]));

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
    let report = one_report(&metered(1).feed(0, &impulse()));

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
    let report = one_report(&metered(1).feed(0, &[1_000i16; 160]));

    assert_eq!(report.peak, 1_000);
    assert_eq!(report.sum, 160_000);
    assert_eq!(report.energy, 160_000_000);
    assert_eq!(report.rms, 1_000);
    assert_eq!(report.clipped_samples, 0);
    assert_eq!(report.dc_offset_windows, 1);
    assert_eq!(report.active_windows, 0);
    assert_eq!(report.silent_windows, 0);
}

/// The negative full-scale sample clips too, and its magnitude is 32,768 (§5.2) — the one place
/// the amplitude domain is not symmetric.
#[test]
fn negative_full_scale_clips_and_peaks_at_the_magnitude_the_spec_states() {
    let report = one_report(&metered(1).feed(0, &[i16::MIN; 160]));

    assert_eq!(report.peak, 32_768, "|−32768| = 32768");
    assert_eq!(report.clipped_samples, 160);
    assert_eq!(report.clipping_windows, 1);
    assert_eq!(report.rms, 32_768);
}

// ---------------------------------------------------------------------------------------------
// Window boundaries and coverage — what `M-59` adds on top of the window facts.
// ---------------------------------------------------------------------------------------------

/// Windows are exactly `W` samples aligned to the stream, and a frame shorter than a window
/// completes nothing.
#[test]
fn windows_are_exactly_w_samples_aligned_to_the_stream_and_span_frame_boundaries() {
    let mut metered = metered(1);

    assert!(
        reports(&metered.feed(0, &[1_000i16; 100])).is_empty(),
        "100 samples do not complete a 160-sample window"
    );

    let observations = metered.feed(1, &[1_000i16; 220]);
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

/// A report names its window and sample coverage, so an application places it on a timeline
/// without having to know the cadence.
#[test]
fn a_report_names_its_window_and_sample_coverage() {
    let report = one_report(&metered(4).feed(0, &[1_000i16; 640]));

    assert_eq!(report.rate, 8_000);
    assert_eq!(report.epoch, 0);
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

/// The cadence bounds how many events a call's audio produces, and a partial period is never
/// reported early.
#[test]
fn the_reporting_cadence_coalesces_windows_and_never_reports_a_partial_period() {
    let mut metered = metered(4);

    let found = reports(&metered.feed(0, &[1_000i16; 160 * 7]));
    assert_eq!(
        found.len(),
        1,
        "seven windows are one full period and a half"
    );
    assert_eq!(found[0].windows, 4);

    let found = reports(&metered.feed(1, &[1_000i16; 160]));
    assert_eq!(found.len(), 1, "the eighth window completes the period");
    assert_eq!(found[0].first_window, 4);
    assert_eq!(found[0].at_sample, 640);
    assert_eq!(found[0].sequence, 1);
}

// ---------------------------------------------------------------------------------------------
// §11.3's silence transition.
// ---------------------------------------------------------------------------------------------

/// CAP-A2 — the silence timeout reaches the application once, at the derived sample count, and
/// re-arms only after a non-silent window.
#[test]
fn cap_a2_the_silence_transition_reaches_the_application_once() {
    let mut metered = metered(1);
    let mut elapsed = Vec::new();

    for sequence in 0..100 {
        for observation in metered.feed(sequence, &[0i16; 160]) {
            if let SignalObservation::SilenceElapsed {
                at_sample,
                rate,
                epoch,
            } = observation
            {
                elapsed.push((at_sample, rate, epoch));
            }
        }
    }

    assert_eq!(
        elapsed,
        vec![(0, 8_000, 0)],
        "16,000 samples of silence at 8,000 Hz is the 2,000 ms timeout, named at the run's first \
         sample, at the rate that sample is counted in, and in the epoch it belongs to"
    );

    for observation in metered.feed(100, &[0i16; 160]) {
        assert!(
            !matches!(observation, SignalObservation::SilenceElapsed { .. }),
            "a still-silent window does not re-fire the timeout"
        );
    }
}

/// A non-silent window clears the run, and the transition can happen again — with the second run's
/// own position.
#[test]
fn a_non_silent_window_re_arms_the_silence_transition() {
    let mut metered = metered(1);

    for sequence in 0..100 {
        metered.feed(sequence, &[0i16; 160]);
    }
    metered.feed(100, &[1_000i16; 160]);

    let mut elapsed = Vec::new();
    for sequence in 101..201 {
        for observation in metered.feed(sequence, &[0i16; 160]) {
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

/// With the timer off, silence produces no transition however long it lasts.
#[test]
fn no_silence_timeout_means_no_silence_transition() {
    let mut metered = Metered::new(SignalReportProfile::new(p8().with_silence_timeout_ms(None)));

    for sequence in 0..200 {
        for observation in metered.feed(sequence, &[0i16; 160]) {
            assert!(
                !matches!(observation, SignalObservation::SilenceElapsed { .. }),
                "the timer is off"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Epochs: a report can never describe an earlier format or an earlier stretch of the call.
// ---------------------------------------------------------------------------------------------

/// CAP-F1 — a format change opens an epoch, and reports after it carry the new rate and coverage.
#[test]
fn cap_f1_a_format_change_opens_an_epoch_and_reports_the_new_rate() {
    let mut metered = metered(1);
    metered.feed(0, &[0i16; 160]);

    let observations = metered.declare_format(16_000);
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

    let report = one_report(&metered.feed(1, &[0i16; 320]));
    assert_eq!(report.rate, 16_000);
    assert_eq!(report.epoch, 1);
    assert_eq!(report.first_window, 0, "the new epoch starts at window 0");
    assert_eq!(report.at_sample, 0);
    assert_eq!(report.samples, 320, "W = ceil(20 · 16000 / 1000)");
}

/// CAP-S1 — a flagged gap discards the partial window unreported, opens an epoch, and the flagged
/// frame's own samples start it.
#[test]
fn cap_s1_a_flagged_gap_opens_a_new_epoch_that_carries_none_of_the_old_samples() {
    let mut metered = metered(1);
    assert!(reports(&metered.feed(0, &[1_000i16; 100])).is_empty());

    let observations = metered.flagged(5, DiscontinuityKind::Loss, &[0i16; 160]);
    assert!(
        matches!(
            observations.first(),
            Some(SignalObservation::Reset {
                cause: ResetCause::Discontinuity {
                    kind: DiscontinuityKind::Loss
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
    assert_eq!(report.sequence, 0, "the report sequence restarts too");
    assert_eq!(
        report.sum, 0,
        "the 100 discarded samples are not in the new epoch's window"
    );
}

/// A reset in the middle of a reporting period discards it: a period cut short covers fewer
/// windows than it would claim, and its windows belong to the epoch that ended.
#[test]
fn a_reset_discards_the_reporting_period_it_interrupts() {
    let mut metered = metered(4);
    assert!(reports(&metered.feed(0, &[1_000i16; 480])).is_empty());

    let observations = metered.flagged(1, DiscontinuityKind::Realign, &[1_000i16; 640]);
    let found = reports(&observations);

    assert_eq!(found.len(), 1, "only the new epoch's full period reports");
    assert_eq!(found[0].epoch, 1);
    assert_eq!(found[0].first_window, 0);
    assert_eq!(found[0].samples, 640);
    assert_eq!(
        found[0].sum, 640_000,
        "none of the three discarded windows is in it"
    );
}

/// Two reporters fed one analyser's observations agree exactly: the reduction is deterministic.
#[test]
fn identical_observations_reduce_identically() {
    let frames: Vec<Vec<i16>> = std::iter::once(alternating())
        .chain((0..11).map(|_| vec![0i16; 160]))
        .collect();

    let run = || {
        let mut metered = metered(2);
        let mut observed = Vec::new();
        for (sequence, samples) in frames.iter().enumerate() {
            observed.extend(metered.feed(sequence as u64, samples));
        }
        observed
    };

    assert_eq!(run(), run());
}

// ---------------------------------------------------------------------------------------------
// Every supported rate, and both supported PCM representations.
// ---------------------------------------------------------------------------------------------

/// Every rate in the linear-PCM domain derives its own window, and the report's coverage is stated
/// in that rate's samples — including both ends of the domain.
#[test]
fn every_supported_rate_reports_its_own_window_coverage() {
    for (rate, window) in [
        (1u32, 1u64),
        (8_000, 160),
        (16_000, 320),
        (44_100, 882),
        (48_000, 960),
        (384_000, 7_680),
    ] {
        let profile = AnalysisProfile::new(AudioDirection::Inbound, rate)
            .with_window_ms(20)
            .with_clip_samples(1);
        let mut metered = Metered::new(SignalReportProfile::new(profile));

        let samples = vec![1_000i16; usize::try_from(window).unwrap()];
        let report = one_report(&metered.feed(0, &samples));
        assert_eq!(report.rate, rate);
        assert_eq!(report.samples, window, "ceil(20 · {rate} / 1000)");
        assert_eq!(report.at_sample, 0);
        assert_eq!(report.peak, 1_000);
    }
}

/// Both linear-PCM representations reach the same reporting boundary, because the PCM boundary
/// converts once and the analyser consumes only the signed form.
#[test]
fn both_supported_pcm_representations_produce_the_declared_numbers() {
    let signed = vec![1_000i16; 160];
    // 1000 is not representable in unsigned 8-bit, whose step is 256; the boundary truncates to
    // the step below, and the metrics describe what actually arrived rather than what was asked
    // for.
    let unsigned = Pcm::from_i16(
        PcmFormat::new(8_000, PcmEncoding::Unsigned8).unwrap(),
        signed.clone(),
    );
    assert!(matches!(unsigned.samples(), PcmSamples::Unsigned8(_)));
    let converted = unsigned.to_i16(8_000).unwrap();

    let from_signed = one_report(&metered(1).feed(0, &signed));
    let from_unsigned = one_report(&metered(1).feed(0, &converted));

    assert_eq!(from_signed.peak, 1_000);
    assert_eq!(from_signed.rms, 1_000);
    assert_eq!(from_unsigned.peak, 768, "the 8-bit step below 1000");
    assert_eq!(from_unsigned.rms, 768);
    assert_eq!(from_unsigned.dc_offset_windows, 1);
}

/// The reported vocabulary is signal content and says so: none of `M-10`'s network-quality names
/// appears in it, so no existing field's meaning is borrowed or changed.
#[test]
fn the_vocabulary_names_signal_content_and_not_network_quality() {
    let names = format!("{:?}", one_report(&metered(1).feed(0, &[0i16; 160])));
    for network in ["loss", "jitter", "round_trip", "mos"] {
        assert!(
            !names.contains(network),
            "a signal report must not borrow M-10's {network} vocabulary: {names}"
        );
    }
}
