//! Property tests for the deterministic signal-metric processor.
//!
//! The vectors in `signal_metrics.rs` cover the cases the specification thought of. These cover
//! the ones it did not: arbitrary sample content at arbitrary rates and cadences, and the extreme
//! amplitudes `docs/specs/call-audio-processing.md` §5.2's width proof is *about*.
//!
//! Two of these tests are the proof itself rather than a sample of it. §4 requires an
//! implementation to debug-assert that overflow is unreachable instead of saturating silently, and
//! Rust's own integer arithmetic panics on overflow in a debug build — which is how these run. A
//! window at the exact corner of the proof (`W = 65,536` of the most negative representable
//! sample) therefore either passes or aborts; there is no third outcome where it quietly wraps.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use proptest::prelude::*;
use sipx_audio::signal::{
    MAX_WINDOW_SAMPLES, SignalDirection, SignalDiscontinuity, SignalFrame, SignalObservation,
    SignalProcessor, SignalProfile,
};

/// The profile whose derived window is exactly `MAX_WINDOW_SAMPLES`, which is the corner every
/// width bound in §5.2 is stated against: `W = ceil(2000 · 32768 / 1000) = 65,536`.
fn widest() -> SignalProfile {
    SignalProfile::new(SignalDirection::Inbound, 32_768)
        .with_window_ms(2_000)
        .with_clip_samples(1)
        .with_windows_per_report(1)
}

fn drain_all(processor: &mut SignalProcessor) -> Vec<SignalObservation> {
    processor.drain().collect()
}

/// Every invariant a report carries, whatever produced it.
fn check_report(observation: &SignalObservation) {
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
        assert!(count <= report.windows, "more windows than the report covers");
    }
    assert!(
        i64::from(report.rms) <= i64::from(report.peak),
        "a quadratic mean cannot exceed the largest magnitude it averages"
    );
    assert!(
        report.sum.unsigned_abs() <= report.samples * 32_768,
        "the DC sum exceeds its own width bound"
    );
}

/// The corner of §5.2's width proof, at both full-scale extremes.
///
/// `W · energy` and `sum²` both reach exactly `2^62` for the most negative sample, which is the
/// largest either quantity can be. Reaching it without a debug assertion or an arithmetic overflow
/// is what makes the proof a fact about this code rather than about the document.
#[test]
fn the_widest_window_of_full_scale_samples_does_not_overflow() {
    for sample in [i16::MIN, i16::MAX] {
        let mut processor = SignalProcessor::new(widest()).unwrap();
        let samples = vec![sample; usize::try_from(MAX_WINDOW_SAMPLES).unwrap()];

        processor
            .process(SignalFrame::new(SignalDirection::Inbound, 0, &samples))
            .unwrap();

        let observations = drain_all(&mut processor);
        let SignalObservation::Report(report) = observations[0] else {
            panic!("expected a report: {observations:?}");
        };
        assert_eq!(report.samples, MAX_WINDOW_SAMPLES);
        assert_eq!(report.clipped_samples, MAX_WINDOW_SAMPLES);
        assert_eq!(report.peak, i32::from(sample.unsigned_abs()));
        assert_eq!(report.active_windows, 0, "a constant signal has no variance");
        for observation in &observations {
            check_report(observation);
        }
    }
}

/// The widest window of alternating extremes: the largest energy a window can hold that is also
/// modulation rather than offset.
#[test]
fn the_widest_window_of_alternating_extremes_does_not_overflow() {
    let mut processor = SignalProcessor::new(widest()).unwrap();
    let samples: Vec<i16> = (0..MAX_WINDOW_SAMPLES)
        .map(|index| if index % 2 == 0 { i16::MAX } else { i16::MIN })
        .collect();

    processor
        .process(SignalFrame::new(SignalDirection::Inbound, 0, &samples))
        .unwrap();

    let observations = drain_all(&mut processor);
    for observation in &observations {
        check_report(observation);
    }
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
        let profile = SignalProfile::new(SignalDirection::Inbound, rate)
            .with_window_ms(window_ms)
            .with_clip_samples(1)
            .with_silence_timeout_ms(Some(50))
            .with_windows_per_report(windows_per_report);
        let Ok(mut processor) = SignalProcessor::new(profile) else {
            // A derived window outside the domain is refused, which is the other correct outcome.
            return Ok(());
        };

        for sequence in 0..frames as u64 {
            processor
                .process(SignalFrame::new(SignalDirection::Inbound, sequence, &samples))
                .unwrap();
            for observation in drain_all(&mut processor) {
                check_report(&observation);
            }
        }
    }

    /// A refused frame changes nothing: the stream that follows it is the stream that would have
    /// followed without it (§7.3).
    #[test]
    fn a_refused_frame_leaves_the_stream_exactly_where_it_stood(
        samples in proptest::collection::vec(any::<i16>(), 1..400),
        skip in 2u64..8,
    ) {
        let profile = SignalProfile::new(SignalDirection::Inbound, 8_000).with_clip_samples(1);

        let mut clean = SignalProcessor::new(profile).unwrap();
        let mut disturbed = SignalProcessor::new(profile).unwrap();

        clean.process(SignalFrame::new(SignalDirection::Inbound, 0, &samples)).unwrap();

        disturbed.process(SignalFrame::new(SignalDirection::Inbound, 0, &samples)).unwrap();
        // An unflagged gap, a repeat, the wrong direction, and an empty frame: all refused.
        prop_assert!(disturbed
            .process(SignalFrame::new(SignalDirection::Inbound, skip, &samples))
            .is_err());
        prop_assert!(disturbed
            .process(SignalFrame::new(SignalDirection::Inbound, 0, &samples))
            .is_err());
        prop_assert!(disturbed
            .process(SignalFrame::new(SignalDirection::Outbound, 1, &samples))
            .is_err());
        prop_assert!(disturbed
            .process(SignalFrame::new(SignalDirection::Inbound, 1, &[]))
            .is_err());

        clean.process(SignalFrame::new(SignalDirection::Inbound, 1, &samples)).unwrap();
        disturbed.process(SignalFrame::new(SignalDirection::Inbound, 1, &samples)).unwrap();

        prop_assert_eq!(drain_all(&mut clean), drain_all(&mut disturbed));
    }

    /// Identical inputs produce identical drains, whatever the input is (§4, CAP-D1).
    #[test]
    fn identical_input_drains_identically(
        samples in proptest::collection::vec(any::<i16>(), 1..600),
        breaks in proptest::collection::vec(
            proptest::option::of(prop_oneof![
                Just(SignalDiscontinuity::Loss),
                Just(SignalDiscontinuity::Overflow),
                Just(SignalDiscontinuity::Realign),
            ]),
            1..6,
        ),
    ) {
        let profile = SignalProfile::new(SignalDirection::Inbound, 8_000)
            .with_clip_samples(1)
            .with_silence_timeout_ms(Some(40));

        let run = || {
            let mut processor = SignalProcessor::new(profile).unwrap();
            let mut observed = Vec::new();
            for (index, discontinuity) in breaks.iter().enumerate() {
                let mut frame =
                    SignalFrame::new(SignalDirection::Inbound, index as u64, &samples);
                if let Some(kind) = discontinuity {
                    frame = frame.with_discontinuity(*kind);
                }
                processor.process(frame).unwrap();
                observed.extend(processor.drain());
            }
            observed
        };

        prop_assert_eq!(run(), run());
    }

    /// The queue is bounded whatever arrives and however long the caller ignores it (§8.3), and
    /// no report ever crosses an epoch boundary.
    #[test]
    fn the_queue_stays_bounded_and_no_report_crosses_an_epoch(
        samples in proptest::collection::vec(any::<i16>(), 160..800),
        capacity in 2u32..=8,
        frames in 4usize..24,
    ) {
        let profile = SignalProfile::new(SignalDirection::Inbound, 8_000)
            .with_clip_samples(1)
            .with_queue_capacity(capacity);
        let mut processor = SignalProcessor::new(profile).unwrap();

        for sequence in 0..frames as u64 {
            let mut frame = SignalFrame::new(SignalDirection::Inbound, sequence, &samples);
            if sequence % 5 == 4 {
                frame = frame.with_discontinuity(SignalDiscontinuity::Loss);
            }
            processor.process(frame).unwrap();
            prop_assert!(
                processor.queued() <= capacity as usize,
                "the queue grew past its bound",
            );
        }

        // Never drained: whatever survived must still be inside one epoch each, and the loss is
        // counted rather than silent.
        for observation in drain_all(&mut processor) {
            check_report(&observation);
            if let SignalObservation::Report(report) = observation {
                prop_assert!(report.epoch <= processor.epoch());
                prop_assert!(report.rate == 8_000);
            }
        }
    }
}
