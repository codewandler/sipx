//! Property tests for signal-metric reporting (`M-59`).
//!
//! The vectors in `signal_metrics.rs` cover the cases the specification thought of. These cover
//! the ones it did not: arbitrary sample content at arbitrary rates and cadences, and the extreme
//! amplitudes `docs/specs/call-audio-processing.md` §5.2's width proof is *about*.
//!
//! Two of these are the proof rather than a sample of it. §4 requires overflow to be unreachable
//! rather than saturated, and Rust's integer arithmetic panics on overflow in a debug build —
//! which is how these run. A period at the corner of the proof (the widest window the domain
//! admits, filled with the most negative representable sample, repeated to the deepest cadence)
//! therefore either passes or aborts; there is no third outcome in which it quietly wraps.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use proptest::prelude::*;
use sipx_audio::analysis::{
    AnalysisFrame, AnalysisProfile, AudioAnalyzer, AudioDirection, DiscontinuityKind,
};
use sipx_audio::signal::{
    MAX_WINDOWS_PER_REPORT, SignalObservation, SignalReportProfile, SignalReporter,
};

/// The analysis profile whose derived window is the largest the domain admits:
/// `ceil(2000 · 32768 / 1000) = 65,536`, which is the corner every width bound in §5.2 is stated
/// against.
fn widest() -> AnalysisProfile {
    AnalysisProfile::new(AudioDirection::Inbound, 32_768)
        .with_window_ms(2_000)
        .with_clip_samples(1)
}

/// One analyser and the reporter summarising it, as the call layer pairs them.
struct Metered {
    analyzer: AudioAnalyzer,
    reporter: SignalReporter,
}

impl Metered {
    fn new(profile: SignalReportProfile) -> Option<Self> {
        Some(Self {
            analyzer: AudioAnalyzer::new(profile.analysis()).ok()?,
            reporter: SignalReporter::new(profile).ok()?,
        })
    }

    fn feed(&mut self, frame: &AnalysisFrame<'_>) -> Vec<SignalObservation> {
        if self.analyzer.process(frame).is_err() {
            return Vec::new();
        }
        let window = self.analyzer.window_samples();
        let reporter = &mut self.reporter;
        self.analyzer
            .drain()
            .filter_map(|observation| reporter.observe(&observation, window))
            .collect()
    }
}

/// Every invariant a report carries, whatever produced it.
fn check(observation: &SignalObservation) {
    let SignalObservation::Report(report) = observation else {
        return;
    };
    assert!(
        (0..=32_768).contains(&report.peak),
        "peak {} is outside the sample-magnitude domain",
        report.peak
    );
    assert!(report.energy >= 0, "a sum of squares is never negative");
    assert!(report.samples > 0, "a report covers at least one window");
    assert!(report.windows > 0);
    assert!(
        report.clipped_samples <= report.samples,
        "more clipped samples than samples"
    );
    for count in [
        report.clipping_windows,
        report.impulsive_windows,
        report.active_windows,
        report.dc_offset_windows,
        report.silent_windows,
    ] {
        assert!(
            count <= report.windows,
            "more windows than the report covers"
        );
    }
    assert!(
        i64::from(report.rms) <= i64::from(report.peak),
        "a quadratic mean cannot exceed the largest magnitude it averages"
    );
    assert!(
        report.sum.unsigned_abs() <= report.samples * 32_768,
        "the DC sum exceeds its own width bound"
    );
    assert!(
        report.at_sample == report.first_window * (report.samples / u64::from(report.windows)),
        "the sample position and the window index must agree"
    );
}

/// The corner of §5.2's width proof, at both full-scale extremes and at the deepest cadence a
/// report may coalesce.
#[test]
fn the_widest_windows_at_the_deepest_cadence_do_not_overflow() {
    const CADENCE: u32 = 8;

    for sample in [i16::MIN, i16::MAX] {
        let mut metered =
            Metered::new(SignalReportProfile::new(widest()).with_windows_per_report(CADENCE))
                .expect("the widest profile is inside every domain");

        let window = vec![sample; 65_536];
        let mut reports = 0usize;
        for sequence in 0..u64::from(CADENCE) {
            for observation in metered.feed(&AnalysisFrame::new(
                AudioDirection::Inbound,
                sequence,
                &window,
            )) {
                check(&observation);
                if let SignalObservation::Report(report) = observation {
                    reports += 1;
                    assert_eq!(report.windows, CADENCE);
                    assert_eq!(report.samples, 65_536 * u64::from(CADENCE));
                    assert_eq!(report.clipped_samples, report.samples);
                    assert_eq!(report.peak, i32::from(sample.unsigned_abs()));
                    assert_eq!(report.rms, u32::from(sample.unsigned_abs()));
                    assert_eq!(
                        report.active_windows, 0,
                        "a constant signal has variance zero"
                    );
                }
            }
        }
        assert_eq!(reports, 1, "the period closes exactly once");
    }
}

/// The widest window of alternating extremes: the largest energy a window can hold that is also
/// modulation rather than offset.
#[test]
fn the_widest_window_of_alternating_extremes_does_not_overflow() {
    let mut metered = Metered::new(SignalReportProfile::new(widest()))
        .expect("the widest profile is inside every domain");
    let window: Vec<i16> = (0..65_536)
        .map(|index| if index % 2 == 0 { i16::MAX } else { i16::MIN })
        .collect();

    let observations = metered.feed(&AnalysisFrame::new(AudioDirection::Inbound, 0, &window));
    assert_eq!(observations.len(), 1, "{observations:?}");
    check(&observations[0]);
    let SignalObservation::Report(report) = observations[0] else {
        panic!("expected a report: {observations:?}");
    };
    assert_eq!(report.peak, 32_768);
    assert_eq!(report.active_windows, 1, "full-scale modulation is active");
}

proptest! {
    /// Arbitrary content at an arbitrary rate, window and cadence: no panic, and every report
    /// invariant holds.
    #[test]
    fn arbitrary_audio_produces_only_well_formed_reports(
        samples in proptest::collection::vec(any::<i16>(), 1..2_000),
        rate in 1u32..=48_000,
        window_ms in 1u32..=100,
        windows_per_report in 1u32..=8,
        frames in 1usize..4,
    ) {
        let analysis = AnalysisProfile::new(AudioDirection::Inbound, rate)
            .with_window_ms(window_ms)
            .with_clip_samples(1)
            .with_silence_timeout_ms(Some(50));
        let profile = SignalReportProfile::new(analysis)
            .with_windows_per_report(windows_per_report);
        // A derived window outside the domain is refused, which is the other correct outcome.
        let Some(mut metered) = Metered::new(profile) else { return Ok(()) };

        for sequence in 0..frames as u64 {
            for observation in metered.feed(
                &AnalysisFrame::new(AudioDirection::Inbound, sequence, &samples),
            ) {
                check(&observation);
            }
        }
    }

    /// Whatever the input, a report's coverage is contiguous, inside one epoch, and never claims
    /// windows a reset or a lost observation took away.
    #[test]
    fn reports_never_claim_coverage_across_a_break(
        samples in proptest::collection::vec(any::<i16>(), 160..900),
        cadence in 1u32..=4,
        breaks in proptest::collection::vec(
            proptest::option::of(prop_oneof![
                Just(DiscontinuityKind::Loss),
                Just(DiscontinuityKind::Overflow),
                Just(DiscontinuityKind::Realign),
            ]),
            1..8,
        ),
        capacity in 2u32..=6,
    ) {
        let analysis = AnalysisProfile::new(AudioDirection::Inbound, 8_000)
            .with_clip_samples(1)
            .with_queue_capacity(capacity);
        let mut metered = Metered::new(
            SignalReportProfile::new(analysis).with_windows_per_report(cadence),
        )
        .unwrap();

        let mut epoch = 0u64;
        let mut seen_in_epoch: Option<u64> = None;
        for (index, discontinuity) in breaks.iter().enumerate() {
            let mut frame =
                AnalysisFrame::new(AudioDirection::Inbound, index as u64, &samples);
            if let Some(kind) = discontinuity {
                frame = frame.with_discontinuity(*kind);
            }
            for observation in metered.feed(&frame) {
                check(&observation);
                match observation {
                    SignalObservation::Reset { epoch: opened, .. } => {
                        prop_assert_eq!(opened, epoch + 1, "epochs advance by one");
                        epoch = opened;
                        seen_in_epoch = None;
                    }
                    SignalObservation::Lost { .. } => seen_in_epoch = None,
                    SignalObservation::Report(report) => {
                        prop_assert_eq!(report.epoch, epoch, "a report names its own epoch");
                        prop_assert_eq!(u64::from(report.windows), u64::from(cadence));
                        if let Some(previous_end) = seen_in_epoch {
                            prop_assert!(
                                report.first_window >= previous_end,
                                "two reports of one epoch must not overlap",
                            );
                        }
                        seen_in_epoch = Some(report.first_window + u64::from(report.windows));
                    }
                    _ => {}
                }
            }
        }
    }

    /// Identical observation streams reduce identically, whatever they contain.
    #[test]
    fn identical_input_reduces_identically(
        samples in proptest::collection::vec(any::<i16>(), 1..600),
        cadence in 1u32..=MAX_WINDOWS_PER_REPORT.min(6),
        frames in 1usize..6,
    ) {
        let analysis = AnalysisProfile::new(AudioDirection::Inbound, 8_000)
            .with_clip_samples(1)
            .with_silence_timeout_ms(Some(40));
        let profile = SignalReportProfile::new(analysis).with_windows_per_report(cadence);

        let run = || {
            let mut metered = Metered::new(profile).unwrap();
            let mut observed = Vec::new();
            for sequence in 0..frames as u64 {
                observed.extend(metered.feed(
                    &AnalysisFrame::new(AudioDirection::Inbound, sequence, &samples),
                ));
            }
            observed
        };

        prop_assert_eq!(run(), run());
    }
}
