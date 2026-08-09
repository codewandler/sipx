//! Hostile call audio against the analyser's declared bounds (`M-61`).
//!
//! `call_audio_analysis.rs` runs the `CAP-*` vectors and `call_audio_calibration.rs` the `CAL-*`
//! ones: those are the inputs `docs/specs/call-audio-processing.md` thought of. This file is the
//! ones it did not — extreme amplitudes, impulses, DC, alternating full swing, arbitrary chunking,
//! sequence gaps, format churn and consumers that never drain — held against §8's bounds rather
//! than against an expected observation.
//!
//! Every assertion here is a *bound*, not a value. What a hostile stream produces is whatever §5.3
//! computes from it; what it may never do is panic, allocate without bound, grow a queue, leave a
//! threshold outside its declared interval, or put raw audio in a diagnostic record.
//!
//! These run in a debug build, where Rust's integer arithmetic panics on overflow rather than
//! wrapping. §4 requires overflow to be *unreachable* rather than saturated, so a corner of §5.2's
//! width proof reached by an input below either passes or aborts — there is no third outcome in
//! which it quietly wraps.
//!
//! The whole-program adversary — format churn, resets and backpressure interleaved by a fuzzer —
//! is `sipx_testkit::call_audio_sequence` and its committed corpus, replayed in
//! `crates/sipx-testkit/tests/call_audio_sequences.rs`. What is here is what needs no driver: one
//! analyser, one property, one bound.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use proptest::prelude::*;
use sipx_audio::analysis::{
    AnalysisFrame, AnalysisProfile, AudioAnalyzer, AudioDirection, CalibrationProfile,
    DiscontinuityKind, MAX_FRAME_SAMPLES, Observation,
};
use sipx_audio::{Pcm, PcmEncoding, PcmFormat};

/// A sample value that is not a count.
///
/// 30,011 is prime, is not a derived window, hangover, silence-timeout or calibration count of any
/// profile below, and is not an index, a capacity or a threshold. Finding its decimal spelling in a
/// diagnostic record can therefore only mean the samples themselves were rendered.
const SENTINEL: i16 = 30_011;

/// §11.1's reference profile.
fn p8() -> AnalysisProfile {
    AnalysisProfile::new(AudioDirection::Inbound, 8_000)
}

/// The same, calibrated by §12.12's `K8`.
fn k8() -> AnalysisProfile {
    p8().with_calibration(Some(CalibrationProfile::new()))
}

fn analyzer(profile: AnalysisProfile) -> AudioAnalyzer {
    AudioAnalyzer::new(profile).expect("a profile inside every §5.1 domain")
}

fn feed(analyzer: &mut AudioAnalyzer, sequence: u64, samples: &[i16]) -> bool {
    analyzer
        .process(&AnalysisFrame::new(
            AudioDirection::Inbound,
            sequence,
            samples,
        ))
        .is_ok()
}

// ---- diagnostics carry identity and counters, never audio ----

/// An ordinary diagnostic record of a frame says what the frame *is*, never what it contains.
///
/// A frame borrows up to 65,536 samples (§3.3). Rendering them into a log record would put raw call
/// audio in the one place operators copy into tickets, and would do it in a line whose length is
/// the frame's — so the same defect is both the retention failure this story exists to close and an
/// unbounded diagnostic. What a record needs is the frame's identity and its counters.
#[test]
fn a_frame_diagnostic_carries_counters_and_no_raw_audio() {
    let samples = vec![SENTINEL; 160];
    let frame = AnalysisFrame::new(AudioDirection::Inbound, 7, &samples);

    let record = format!("{frame:?}");

    assert!(
        !record.contains("30011"),
        "a frame's diagnostic rendering carries no sample value: {record}"
    );
    assert!(
        record.contains("160"),
        "how much audio there was is a counter and belongs in the record: {record}"
    );
    assert!(
        record.contains('7'),
        "and so does which frame it was: {record}"
    );
    assert!(
        record.len() < 200,
        "a record whose length is the frame's is unbounded diagnostics: {record}"
    );
}

/// The same, for the profile identity an ordinary record needs beside it.
#[test]
fn an_analyser_diagnostic_carries_profile_identity_and_counters() {
    let mut analyzer = analyzer(p8());
    let samples = vec![SENTINEL; 160];
    analyzer
        .process(&AnalysisFrame::new(AudioDirection::Inbound, 0, &samples))
        .unwrap();

    let record = format!("{}", analyzer.thresholds());

    assert!(
        record.contains("inbound"),
        "which side of the call it measures: {record}"
    );
    assert!(record.contains("8000"), "at what rate: {record}");
    assert!(record.contains("160"), "over what window: {record}");
    assert!(
        record.contains("2048"),
        "and against what threshold: {record}"
    );
    assert!(
        !record.contains("30011"),
        "and no sample of the audio it measured: {record}"
    );
}

/// The whole of what an analyser can be asked to render, after a long hostile call, is counters.
///
/// The strong form of the row: not "this record happens to omit the audio" but "there is no audio
/// to render". Every diagnostic reachable from an analyser — its own `Debug`, its snapshot's
/// `Display` and `Debug`, and each observation it produced — is swept for the sentinel the call was
/// full of.
#[test]
fn nothing_reachable_from_a_used_analyser_renders_the_audio_it_measured() {
    let mut analyzer = analyzer(k8().with_queue_capacity(4_096));
    let loud = vec![SENTINEL; 160];
    let modulated: Vec<i16> = (0..160)
        .map(|index| if index % 2 == 0 { SENTINEL } else { -SENTINEL })
        .collect();
    let mut records = Vec::new();
    for sequence in 0..40u64 {
        let samples = if sequence % 2 == 0 { &loud } else { &modulated };
        assert!(feed(&mut analyzer, sequence, samples));
        for observation in analyzer.drain() {
            records.push(format!("{observation:?}"));
        }
    }
    records.push(format!("{analyzer:?}"));
    records.push(format!("{}", analyzer.thresholds()));
    records.push(format!("{:?}", analyzer.thresholds()));

    // The accumulators §5.3 publishes are sums over a window, so the sentinel's digits may appear
    // inside a much larger number. What may not appear is a *sample*: a rendered slice, or the
    // sentinel standing where the contract publishes one value per sample.
    for record in &records {
        assert!(
            !record.contains("[30011"),
            "no record may carry a rendered sample slice: {record}"
        );
        assert!(
            !record.contains("30011, 30011"),
            "nor a run of them: {record}"
        );
    }
    assert!(
        records.iter().any(|record| record.contains("index")),
        "the sweep is only worth anything if it saw the observations"
    );
}

// ---- §8's bounds under long silence, permanent activity and backpressure ----

/// A consumer that never drains costs a fixed number of slots, whatever the audio does.
///
/// §8.3's whole promise: an enqueue that would exceed capacity coalesces the newest retained entry
/// into counted loss rather than blocking the caller or growing past the configured depth. Two
/// hostile streams — one silent forever, one active forever — at three depths, and none of them may
/// make the queue hold one slot more than it was built with.
#[test]
fn a_consumer_that_never_drains_costs_a_fixed_number_of_slots() {
    let modulated: Vec<i16> = (0..160)
        .map(|index| if index % 2 == 0 { 8_192 } else { -8_192 })
        .collect();
    let silence = [0i16; 160];

    for capacity in [2u32, 64, 4_096] {
        for (shape, samples) in [("silence", &silence[..]), ("activity", &modulated[..])] {
            let mut analyzer = analyzer(k8().with_queue_capacity(capacity));
            let slots = usize::try_from(capacity).unwrap();
            for sequence in 0..2_000u64 {
                assert!(feed(&mut analyzer, sequence, samples));
                assert!(
                    analyzer.queued() <= slots,
                    "{capacity}/{shape}: {} observations in {capacity} slots at frame {sequence}",
                    analyzer.queued()
                );
            }
            let drained: Vec<Observation> = analyzer.drain().collect();
            assert!(
                drained.len() <= slots,
                "{capacity}/{shape}: a drain yielded {} of {capacity}",
                drained.len()
            );
            assert_eq!(
                analyzer.queued(),
                0,
                "{capacity}/{shape}: a drain empties it"
            );
        }
    }
}

/// Work per frame is the frame's own length and nothing else (§8.2).
///
/// The observable form of *"there is no path whose cost depends on call length or on past input"*:
/// a frame completes exactly the windows its samples fill, and each completed window enqueues at
/// most the four observations §6 and §12.7 order — so the observations a frame can produce are a
/// function of its size, not of the three thousand frames before it.
#[test]
fn work_per_frame_is_the_frames_own_length_and_nothing_else() {
    let mut analyzer = analyzer(k8().with_queue_capacity(4_096));
    let window = u64::from(analyzer.window_samples());
    // A length that is not a whole number of windows, so a partial window is always carried.
    let samples = vec![i16::MIN; 97];
    let mut carried = 0u64;
    for sequence in 0..3_000u64 {
        assert!(feed(&mut analyzer, sequence, &samples));
        let filled = carried + 97;
        let completed = filled / window;
        carried = filled % window;

        let drained = analyzer.drain().count();
        let ceiling = usize::try_from(completed * 4 + 1).unwrap();
        assert!(
            drained <= ceiling,
            "frame {sequence} completed {completed} windows and produced {drained} observations"
        );
    }
}

/// Long silence reports elapsed silence once and then says nothing, however long it runs (§6).
#[test]
fn silence_that_never_ends_reports_itself_exactly_once() {
    let mut analyzer = analyzer(p8().with_queue_capacity(4_096));
    let mut elapsed = 0usize;
    for sequence in 0..2_000u64 {
        assert!(feed(&mut analyzer, sequence, &[0i16; 160]));
        elapsed += analyzer
            .drain()
            .filter(|observation| matches!(observation, Observation::SilenceElapsed { .. }))
            .count();
    }
    assert_eq!(
        elapsed, 1,
        "a timer that re-armed on its own would report a silent call forever"
    );
}

// ---- refusals move nothing ----

proptest! {
    /// A refused frame leaves the stream exactly where it stood (§7.3), whatever it carried.
    #[test]
    fn a_refused_frame_moves_nothing(
        sequence in 0u64..8,
        oversized in any::<bool>(),
        outbound in any::<bool>(),
    ) {
        let mut analyzer = analyzer(k8());
        for frame in 0..4u64 {
            prop_assert!(feed(&mut analyzer, frame, &[1_000i16; 160]));
        }
        let before = (
            analyzer.sample_rate(),
            analyzer.window_samples(),
            analyzer.activation_amplitude(),
            analyzer.queued(),
            analyzer.is_voiced(),
        );

        let samples = if oversized {
            vec![i16::MAX; MAX_FRAME_SAMPLES + 1]
        } else {
            Vec::new()
        };
        let direction = if outbound {
            AudioDirection::Outbound
        } else {
            AudioDirection::Inbound
        };
        let refused = analyzer.process(&AnalysisFrame::new(direction, sequence, &samples));
        prop_assert!(refused.is_err(), "{refused:?}");

        prop_assert_eq!(
            before,
            (
                analyzer.sample_rate(),
                analyzer.window_samples(),
                analyzer.activation_amplitude(),
                analyzer.queued(),
                analyzer.is_voiced(),
            )
        );
        // And the stream continues from where it stood, rather than from where the refusal was.
        prop_assert!(feed(&mut analyzer, 4, &[1_000i16; 160]));
    }

    /// A refused format change never half-applies (§7.2): the previous rate stays in force and
    /// every count derived from it is untouched.
    #[test]
    fn a_refused_format_change_leaves_the_previous_format_in_force(rate in 384_001u32..1_000_000) {
        let mut analyzer = analyzer(k8());
        prop_assert!(feed(&mut analyzer, 0, &[1_000i16; 160]));
        let before = (
            analyzer.sample_rate(),
            analyzer.window_samples(),
            analyzer.hangover_samples(),
            analyzer.thresholds().calibration_samples(),
        );

        prop_assert!(analyzer.declare_format(rate).is_err());
        prop_assert!(analyzer.declare_format(0).is_err());

        prop_assert_eq!(
            before,
            (
                analyzer.sample_rate(),
                analyzer.window_samples(),
                analyzer.hangover_samples(),
                analyzer.thresholds().calibration_samples(),
            )
        );
    }

    /// The same audio, chunked differently, is the same measurement.
    ///
    /// Windows are aligned to the stream position rather than to the frame (§5.2), so where a
    /// caller chose to cut its buffers may not change one observation. This is the property behind
    /// every fixture in `call_audio_analysis.rs` that feeds 160 samples at a time: if it did not
    /// hold, those fixtures would be about the chunking and not about the audio.
    #[test]
    fn arbitrary_chunking_produces_identical_observations(
        samples in proptest::collection::vec(any::<i16>(), 480..1_200),
        first in 1usize..400,
        second in 1usize..400,
    ) {
        prop_assert_eq!(
            observations_chunked(&samples, first),
            observations_chunked(&samples, second)
        );
    }
}

/// Drive one analyser with `samples` cut into frames of `chunk`, and collect everything it said.
fn observations_chunked(samples: &[i16], chunk: usize) -> Vec<Observation> {
    let mut analyzer = analyzer(k8().with_queue_capacity(4_096));
    let mut drained = Vec::new();
    for (sequence, frame) in samples.chunks(chunk).enumerate() {
        let sequence = u64::try_from(sequence).unwrap_or(0);
        assert!(feed(&mut analyzer, sequence, frame));
        drained.extend(analyzer.drain());
    }
    drained
}

// ---- every supported PCM format, and the corners of the width proof ----

/// Every `(rate, encoding)` the application boundary supports reaches the analyser and is measured.
///
/// "Supported PCM format" is [`PcmFormat`], and its eight-bit half arrives only after the
/// boundary's own conversion — the route `M-43` gives the seam. A format that panicked on the way
/// in would be a hostile-input defect one layer above the analyser, where nothing else looks for
/// it.
#[test]
fn every_supported_pcm_format_reaches_the_analyser_and_is_measured() {
    let mut measured = 0usize;
    for rate in [1u32, 8_000, 11_025, 16_000, 44_100, 48_000, 96_000, 384_000] {
        for encoding in [PcmEncoding::Unsigned8, PcmEncoding::Signed16] {
            let format = PcmFormat::new(rate, encoding).expect("inside the boundary's domain");
            // The conversion is a rate change, so a source length is not a frame length: 2,000
            // samples at 1 Hz become sixteen million at 8,000 and the analyser refuses them for
            // length (§7.3), which would make this test about the ceiling instead of the format.
            let source_samples = (2_000u64 * u64::from(rate) / 8_000).clamp(1, 2_000);
            let source: Vec<i16> = (0..source_samples)
                .map(|index| if index % 2 == 0 { i16::MAX } else { i16::MIN })
                .collect();
            let converted = Pcm::from_i16(format, source)
                .to_i16(8_000)
                .expect("the boundary converts into the analyser's declared format");
            if converted.is_empty() {
                continue;
            }
            let mut analyzer = analyzer(k8().with_queue_capacity(4_096));
            assert!(
                feed(&mut analyzer, 0, &converted),
                "{rate} Hz {encoding:?} was refused"
            );
            measured += analyzer.drain().count();
        }
    }
    assert!(measured > 0, "nothing was measured at any format");
}

/// The corner of §5.2's width proof, with calibration configured to move as far and as often as its
/// own domain allows.
///
/// The widest window the domain admits, filled with the most negative representable sample, under a
/// margin and a step at the top of their ranges and a freeze bound of one — every intermediate at
/// its maximum at once. In a debug build this either passes or aborts on overflow; there is no
/// third outcome in which it wraps.
#[test]
fn the_widest_window_of_extremes_under_the_widest_calibration_does_not_overflow() {
    let calibration = CalibrationProfile::new()
        .with_calibration_ms(1)
        .with_update_ms(1)
        .with_margin_amplitude(32_767)
        .with_floor_amplitude(1)
        .with_ceiling_amplitude(32_767)
        .with_max_step_amplitude(32_767)
        .with_freeze_limit_ms(Some(1));
    let profile = AnalysisProfile::new(AudioDirection::Inbound, 32_768)
        .with_window_ms(2_000)
        .with_activation_amplitude(1)
        .with_clip_samples(1)
        .with_queue_capacity(4_096)
        .with_calibration(Some(calibration));
    let mut analyzer = analyzer(profile);
    assert_eq!(analyzer.window_samples(), 65_536, "the widest §5.1 admits");

    let extremes: Vec<i16> = (0..MAX_FRAME_SAMPLES)
        .map(|index| if index % 2 == 0 { i16::MAX } else { i16::MIN })
        .collect();
    let constant = vec![i16::MIN; MAX_FRAME_SAMPLES];
    for sequence in 0..6u64 {
        let samples = if sequence % 2 == 0 {
            &extremes
        } else {
            &constant
        };
        assert!(feed(&mut analyzer, sequence, samples));
        for observation in analyzer.drain() {
            if let Observation::Window { peak, energy, .. } = observation {
                assert!((0..=32_768).contains(&peak), "{peak}");
                assert!(energy >= 0, "{energy}");
            }
        }
    }
    let activation = analyzer.activation_amplitude();
    assert!(
        (1..=32_767).contains(&activation),
        "the threshold stayed inside its declared interval: {activation}"
    );
}

/// A discontinuity storm cannot make a position, an index or a threshold leave its domain.
///
/// The lifecycle half of the hostile-input row: a seam that flags a break on every frame restarts
/// the epoch on every frame, so nothing ever completes a window and every counter is asked to
/// restart at its own boundary.
#[test]
fn a_break_on_every_frame_leaves_every_count_inside_its_domain() {
    let mut analyzer = analyzer(k8().with_queue_capacity(4_096));
    let kinds = [
        DiscontinuityKind::Loss,
        DiscontinuityKind::Overflow,
        DiscontinuityKind::Realign,
    ];
    let modulated: Vec<i16> = (0..159)
        .map(|index| if index % 2 == 0 { 8_192 } else { -8_192 })
        .collect();
    let calibration = CalibrationProfile::new();
    for sequence in 0..1_000u64 {
        let kind = kinds[usize::try_from(sequence).unwrap() % kinds.len()];
        let frame = AnalysisFrame::new(AudioDirection::Inbound, sequence, &modulated)
            .with_discontinuity(kind);
        analyzer.process(&frame).expect("a flagged frame is legal");
        let activation = analyzer.activation_amplitude();
        assert!(
            (calibration.floor_amplitude()..=calibration.ceiling_amplitude()).contains(&activation),
            "frame {sequence}: {activation}"
        );
        let _ = analyzer.drain().count();
    }
    // Every epoch was 159 samples long and the window is 160, so no window ever completed and the
    // analyser is exactly where it started.
    assert!(!analyzer.is_voiced());
    assert_eq!(analyzer.thresholds().updates(), 0);
}
