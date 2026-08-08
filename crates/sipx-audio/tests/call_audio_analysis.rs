//! The `CAP-*` vectors of [`docs/specs/call-audio-processing.md`](../../../docs/specs/call-audio-processing.md) §11.
//!
//! `M-57` wrote these before any implementation existed, which is the point: the contract's central
//! promise is that identical inputs produce identical observations on every machine, so a fixture is
//! evidence rather than a statistical argument. Each test below names the vector it replays and the
//! section that defines the expectation, and every number in it is copied from the specification
//! rather than read off a run.
//!
//! The corpus runs against the crate's public API only — an integration test, so a vector that needs
//! something the published surface cannot express is a finding about the surface.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_audio::PcmError;
use sipx_audio::analysis::{
    AnalysisFrame, AnalysisProfile, AudioAnalyzer, AudioDirection, DiscontinuityKind, FrameError,
    Observation, ResetCause, VoiceEndCause,
};

/// §11.1's reference profile `P8`: inbound, 8,000 Hz, 20 ms windows (`W = 160`).
fn p8() -> AnalysisProfile {
    AnalysisProfile::new(AudioDirection::Inbound, 8_000)
}

fn analyzer() -> AudioAnalyzer {
    AudioAnalyzer::new(p8()).unwrap()
}

/// One `P8` frame: inbound, in sequence, unflagged.
fn frame(sequence: u64, samples: &[i16]) -> AnalysisFrame<'_> {
    AnalysisFrame::new(AudioDirection::Inbound, sequence, samples)
}

/// CAP-W2's pattern: one window of alternating full-swing modulation.
fn alternating() -> Vec<i16> {
    (0..160)
        .map(|index| if index % 2 == 0 { 8_192 } else { -8_192 })
        .collect()
}

/// Feed one frame and take everything it produced.
fn observe(analyzer: &mut AudioAnalyzer, sequence: u64, samples: &[i16]) -> Vec<Observation> {
    analyzer.process(&frame(sequence, samples)).unwrap();
    analyzer.drain().collect()
}

/// The single `Window` observation a one-window frame produces.
fn window_of(observations: &[Observation]) -> Observation {
    match observations.first() {
        Some(observation @ Observation::Window { .. }) => observation.clone(),
        other => panic!("expected a window observation first, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------------
// §11.2 Window facts
// ---------------------------------------------------------------------------------------------

/// CAP-W1 — silence measures nothing but silence.
#[test]
fn cap_w1_a_zero_window_is_silent_and_nothing_else() {
    let mut analyzer = analyzer();
    let observations = observe(&mut analyzer, 0, &[0i16; 160]);

    assert_eq!(
        window_of(&observations),
        Observation::Window {
            index: 0,
            peak: 0,
            sum: 0,
            energy: 0,
            clipped: 0,
            clipping: false,
            impulsive: false,
            active: false,
            dc_offset: false,
            silent: true,
        }
    );
    assert_eq!(observations.len(), 1, "silence alone is one fact");
}

/// CAP-W2 — the DC-free variance form is what makes modulation voice.
#[test]
fn cap_w2_alternating_full_swing_is_active_and_opens_voice() {
    let mut analyzer = analyzer();
    let observations = observe(&mut analyzer, 0, &alternating());

    assert_eq!(
        window_of(&observations),
        Observation::Window {
            index: 0,
            peak: 8_192,
            sum: 0,
            energy: 10_737_418_240,
            clipped: 0,
            clipping: false,
            impulsive: false,
            active: true,
            dc_offset: false,
            silent: false,
        }
    );
    assert_eq!(
        observations[1],
        Observation::VoiceStarted { at_sample: 0 },
        "§6: an active window in `Inactive` opens voice at the window's first sample"
    );
}

/// CAP-W3 — a stuck DAC at full scale has the energy of speech and the variance of a constant.
#[test]
fn cap_w3_a_constant_at_full_scale_clips_and_offsets_but_is_not_voice() {
    let mut analyzer = analyzer();
    let observations = observe(&mut analyzer, 0, &[32_767i16; 160]);

    assert_eq!(
        window_of(&observations),
        Observation::Window {
            index: 0,
            peak: 32_767,
            sum: 5_242_720,
            energy: 171_788_206_240,
            clipped: 160,
            clipping: true,
            impulsive: false,
            active: false,
            dc_offset: true,
            silent: false,
        }
    );
    assert_eq!(
        observations.len(),
        1,
        "`W·energy − sum²` is exactly 0 here, so no voice transition follows"
    );
}

/// CAP-W4 — the impulse exclusion runs first, so one click is not a voice onset.
#[test]
fn cap_w4_a_single_full_scale_sample_is_impulsive_and_not_active() {
    let mut analyzer = analyzer();
    let mut samples = [0i16; 160];
    samples[40] = 32_767;
    let observations = observe(&mut analyzer, 0, &samples);

    assert_eq!(
        window_of(&observations),
        Observation::Window {
            index: 0,
            peak: 32_767,
            sum: 32_767,
            energy: 1_073_676_289,
            clipped: 1,
            clipping: false,
            impulsive: true,
            active: false,
            dc_offset: false,
            silent: false,
        }
    );
    assert_eq!(observations.len(), 1, "an impulse opens no voice");
}

/// CAP-W5 — a DC-biased capture reports its offset and stays out of the activity machine.
#[test]
fn cap_w5_a_constant_offset_is_dc_and_not_active() {
    let mut analyzer = analyzer();
    let observations = observe(&mut analyzer, 0, &[1_000i16; 160]);

    assert_eq!(
        window_of(&observations),
        Observation::Window {
            index: 0,
            peak: 1_000,
            sum: 160_000,
            energy: 160_000_000,
            clipped: 0,
            clipping: false,
            impulsive: false,
            active: false,
            dc_offset: true,
            silent: false,
        }
    );
    assert_eq!(observations.len(), 1, "variance 0 is not voice");
}

// ---------------------------------------------------------------------------------------------
// §11.3 Activity, hangover, silence timeout
// ---------------------------------------------------------------------------------------------

/// CAP-A1 — voice ends one derived hangover after the last active window, at that window's end.
#[test]
fn cap_a1_voice_ends_at_the_hangover_and_not_before() {
    let mut analyzer = analyzer();

    let opening = observe(&mut analyzer, 0, &alternating());
    assert_eq!(opening[1], Observation::VoiceStarted { at_sample: 0 });

    // Windows 1..=9 are inactive but inside the 1,600-sample hangover.
    for sequence in 1..=9u64 {
        let quiet = observe(&mut analyzer, sequence, &[0i16; 160]);
        assert_eq!(
            quiet.len(),
            1,
            "window {sequence} is inside the hangover: only its `Window` fact is emitted"
        );
    }

    // Window 10 is the tenth inactive window: the run reaches 10 · 160 = 1,600.
    let closing = observe(&mut analyzer, 10, &[0i16; 160]);
    assert_eq!(
        closing[1],
        Observation::VoiceEnded {
            at_sample: 160,
            cause: VoiceEndCause::Hangover,
        },
        "voice ends at the end of the last active window, not where the hangover expired"
    );

    // The silent run is 1,760 samples: well short of the 16,000-sample timeout.
    let quiet = observe(&mut analyzer, 11, &[0i16; 160]);
    assert_eq!(
        quiet.len(),
        1,
        "no `SilenceElapsed` before the timeout count"
    );
}

/// CAP-A2 — the silence timeout fires once, at the first sample of the run.
#[test]
fn cap_a2_the_silence_timeout_fires_exactly_once() {
    let mut analyzer = analyzer();

    for sequence in 0..99u64 {
        let quiet = observe(&mut analyzer, sequence, &[0i16; 160]);
        assert_eq!(quiet.len(), 1, "window {sequence} is short of the timeout");
    }

    // Window 99 takes the silent run to 100 · 160 = 16,000.
    let elapsed = observe(&mut analyzer, 99, &[0i16; 160]);
    assert_eq!(
        elapsed[1],
        Observation::SilenceElapsed { at_sample: 0 },
        "the timeout names the first sample of the run"
    );

    let after = observe(&mut analyzer, 100, &[0i16; 160]);
    assert_eq!(
        after.len(),
        1,
        "the timer re-arms only after a non-silent window or a reset"
    );
}

/// CAP-A3 — a reset cuts voice before it announces the new epoch.
#[test]
fn cap_a3_a_requested_reset_cuts_active_voice_first() {
    let mut analyzer = analyzer();
    let opening = observe(&mut analyzer, 0, &alternating());
    assert_eq!(opening[1], Observation::VoiceStarted { at_sample: 0 });

    analyzer.reset();
    let observations: Vec<Observation> = analyzer.drain().collect();
    assert_eq!(
        observations,
        vec![
            Observation::VoiceEnded {
                at_sample: 160,
                cause: VoiceEndCause::Cut,
            },
            Observation::Reset {
                cause: ResetCause::Requested,
            },
        ]
    );
}

/// CAP-D1 — two processors, one configuration, one input: byte-identical drains.
#[test]
fn cap_d1_two_processors_agree_exactly() {
    let mut left = analyzer();
    let mut right = analyzer();

    let mut left_seen = Vec::new();
    let mut right_seen = Vec::new();
    for sequence in 0..12u64 {
        let samples = if sequence == 0 {
            alternating()
        } else {
            vec![0i16; 160]
        };
        left_seen.extend(observe(&mut left, sequence, &samples));
        right_seen.extend(observe(&mut right, sequence, &samples));
    }

    assert_eq!(left_seen, right_seen);
}

// ---------------------------------------------------------------------------------------------
// §11.4 Format changes
// ---------------------------------------------------------------------------------------------

/// CAP-F1 — a declared rate re-derives every count and opens a new epoch.
#[test]
fn cap_f1_a_format_change_resets_and_re_derives_the_window() {
    let mut analyzer = analyzer();
    let _ = observe(&mut analyzer, 0, &[0i16; 160]);

    analyzer.declare_format(16_000).unwrap();
    assert_eq!(
        analyzer.drain().collect::<Vec<_>>(),
        vec![Observation::Reset {
            cause: ResetCause::FormatChange { rate: 16_000 },
        }]
    );
    assert_eq!(analyzer.window_samples(), 320, "20 ms at 16,000 Hz");

    let observations = observe(&mut analyzer, 1, &[0i16; 320]);
    assert_eq!(
        window_of(&observations),
        Observation::Window {
            index: 0,
            peak: 0,
            sum: 0,
            energy: 0,
            clipped: 0,
            clipping: false,
            impulsive: false,
            active: false,
            dc_offset: false,
            silent: true,
        },
        "the 320 samples complete the new epoch's window 0"
    );
}

/// CAP-F2 — a malformed format change never half-applies.
#[test]
fn cap_f2_an_unsupported_rate_leaves_the_declared_format_in_force() {
    let mut analyzer = analyzer();

    assert_eq!(
        analyzer.declare_format(0),
        Err(PcmError::UnsupportedSampleRate(0).into())
    );
    assert_eq!(
        analyzer.declare_format(384_001),
        Err(PcmError::UnsupportedSampleRate(384_001).into())
    );
    assert_eq!(analyzer.drain().count(), 0, "a refusal emits nothing");
    assert_eq!(analyzer.window_samples(), 160, "8,000 Hz remains in force");

    let observations = observe(&mut analyzer, 0, &[0i16; 160]);
    assert_eq!(
        observations.len(),
        1,
        "the next in-sequence frame is accepted"
    );
}

/// CAP-F3 — the derivation rounds up, never truncating a window below the duration it covers.
#[test]
fn cap_f3_the_window_derivation_rounds_up() {
    let mut analyzer = analyzer();
    analyzer.declare_format(8_193).unwrap();
    assert_eq!(analyzer.window_samples(), 164, "ceil(20 · 8,193 / 1000)");
}

// ---------------------------------------------------------------------------------------------
// §11.5 Sequence and discontinuity
// ---------------------------------------------------------------------------------------------

/// CAP-S1 — a flagged gap discards the partial window and opens a new epoch before its samples.
#[test]
fn cap_s1_a_flagged_gap_resets_before_the_frame_it_flags() {
    let mut analyzer = analyzer();
    let partial = observe(&mut analyzer, 0, &[0i16; 100]);
    assert!(
        partial.is_empty(),
        "100 samples complete no 160-sample window"
    );

    analyzer
        .process(&frame(5, &[0i16; 160]).with_discontinuity(DiscontinuityKind::Loss))
        .unwrap();
    let observations: Vec<Observation> = analyzer.drain().collect();
    assert_eq!(
        observations[0],
        Observation::Reset {
            cause: ResetCause::Discontinuity {
                kind: DiscontinuityKind::Loss,
            },
        },
        "the partial window is discarded unemitted and the reset precedes the samples"
    );
    assert!(
        matches!(observations[1], Observation::Window { index: 0, .. }),
        "the flagged frame's samples complete the new epoch's window 0: {observations:?}"
    );

    analyzer.process(&frame(6, &[0i16; 160])).unwrap();
}

/// CAP-S2 — an unflagged gap is a broken upstream, and refusing it changes nothing.
#[test]
fn cap_s2_an_unflagged_gap_is_refused_without_state_change() {
    let mut analyzer = analyzer();
    let _ = observe(&mut analyzer, 0, &[0i16; 160]);

    assert_eq!(
        analyzer.process(&frame(2, &[0i16; 160])),
        Err(FrameError::MalformedSequence)
    );
    assert_eq!(analyzer.drain().count(), 0, "a refusal emits nothing");

    let observations = observe(&mut analyzer, 1, &[0i16; 160]);
    assert!(
        matches!(
            window_of(&observations),
            Observation::Window { index: 1, .. }
        ),
        "the stream continues exactly where it stood: {observations:?}"
    );
}

/// CAP-S3 — sequence is strictly monotonic, flagged or not.
#[test]
fn cap_s3_a_repeated_sequence_is_refused_either_way() {
    let mut analyzer = analyzer();
    let _ = observe(&mut analyzer, 0, &[0i16; 160]);

    assert_eq!(
        analyzer.process(&frame(0, &[0i16; 160])),
        Err(FrameError::MalformedSequence)
    );
    assert_eq!(
        analyzer.process(&frame(0, &[0i16; 160]).with_discontinuity(DiscontinuityKind::Realign)),
        Err(FrameError::MalformedSequence)
    );
}

/// CAP-S4 — a flagged frame need not skip a sequence number.
#[test]
fn cap_s4_a_flagged_frame_in_sequence_still_resets() {
    let mut analyzer = analyzer();
    let _ = observe(&mut analyzer, 0, &[0i16; 160]);

    analyzer
        .process(&frame(1, &[0i16; 160]).with_discontinuity(DiscontinuityKind::Overflow))
        .unwrap();
    let observations: Vec<Observation> = analyzer.drain().collect();
    assert_eq!(
        observations[0],
        Observation::Reset {
            cause: ResetCause::Discontinuity {
                kind: DiscontinuityKind::Overflow,
            },
        }
    );
}

// ---------------------------------------------------------------------------------------------
// §11.6 Queue bound
// ---------------------------------------------------------------------------------------------

/// CAP-Q1 — an undersized queue loses observations visibly and deterministically.
#[test]
fn cap_q1_an_overflowing_queue_coalesces_into_a_counted_marker() {
    let mut analyzer = AudioAnalyzer::new(p8().with_queue_capacity(2)).unwrap();
    analyzer.process(&frame(0, &[0i16; 480])).unwrap();

    let observations: Vec<Observation> = analyzer.drain().collect();
    assert_eq!(observations.len(), 2);
    assert!(matches!(
        observations[0],
        Observation::Window { index: 0, .. }
    ));
    assert_eq!(observations[1], Observation::Lost { count: 2 });
}

// ---------------------------------------------------------------------------------------------
// §11.7 Refused frames
// ---------------------------------------------------------------------------------------------

/// CAP-N1 — zero samples measure nothing, and a silent no-op would hide a broken seam.
#[test]
fn cap_n1_an_empty_frame_is_refused_without_state_change() {
    let mut analyzer = analyzer();
    let _ = observe(&mut analyzer, 0, &[0i16; 100]);

    assert_eq!(
        analyzer.process(&frame(1, &[])),
        Err(FrameError::MalformedFrame)
    );

    // The position and the sequence expectation are both untouched: 60 more samples still
    // complete window 0 of this epoch, and sequence 1 is still the next one accepted.
    let observations = observe(&mut analyzer, 1, &[0i16; 60]);
    assert!(matches!(
        window_of(&observations),
        Observation::Window { index: 0, .. }
    ));
}

/// CAP-N2 — the per-frame CPU ceiling is a contract, not a suggestion.
#[test]
fn cap_n2_an_oversized_frame_is_refused() {
    let mut analyzer = analyzer();
    assert_eq!(
        analyzer.process(&frame(0, &vec![0i16; 65_537])),
        Err(FrameError::MalformedFrame)
    );
}

/// CAP-N3 — per-direction analysis state never interleaves.
#[test]
fn cap_n3_the_other_direction_is_refused() {
    let mut analyzer = analyzer();
    assert_eq!(
        analyzer.process(&AnalysisFrame::new(
            AudioDirection::Outbound,
            0,
            &[0i16; 160]
        )),
        Err(FrameError::DirectionMismatch)
    );
}

// ---------------------------------------------------------------------------------------------
// §11.8 Refused configurations
// ---------------------------------------------------------------------------------------------

/// CAP-C1 — a violated domain is a typed error naming the field.
#[test]
fn cap_c1_a_zero_window_names_its_field() {
    let error = AudioAnalyzer::new(p8().with_window_ms(0)).unwrap_err();
    assert!(
        error.to_string().contains("window_ms"),
        "the refusal names the field: {error}"
    );
}

/// CAP-C2 — a derived window past the arithmetic bound is refused, never clamped.
#[test]
fn cap_c2_an_oversized_derived_window_is_refused() {
    let profile = AnalysisProfile::new(AudioDirection::Inbound, 384_000).with_window_ms(200);
    let error = AudioAnalyzer::new(profile).unwrap_err();
    assert!(
        error.to_string().contains("76800"),
        "the refusal reports the derived length it will not clamp: {error}"
    );
}

/// CAP-C3 — the queue domain is the seam's, and both ends of it are refused.
#[test]
fn cap_c3_a_queue_outside_its_domain_names_its_field() {
    for capacity in [1u32, 4_097] {
        let error = AudioAnalyzer::new(p8().with_queue_capacity(capacity)).unwrap_err();
        assert!(
            error.to_string().contains("queue_capacity"),
            "the refusal names the field: {error}"
        );
    }
}

/// CAP-C4 — the rate refusal is the linear-PCM boundary's own type, reused rather than re-minted.
#[test]
fn cap_c4_an_unsupported_rate_reuses_the_pcm_refusal() {
    for rate in [0u32, 384_001] {
        let error =
            AudioAnalyzer::new(AnalysisProfile::new(AudioDirection::Inbound, rate)).unwrap_err();
        assert_eq!(error, PcmError::UnsupportedSampleRate(rate).into());
    }
}

/// CAP-C5 — a zero timeout would fire before any silence existed.
#[test]
fn cap_c5_a_zero_silence_timeout_is_refused() {
    let error = AudioAnalyzer::new(p8().with_silence_timeout_ms(Some(0))).unwrap_err();
    assert!(
        error.to_string().contains("silence_timeout_ms"),
        "the refusal names the field: {error}"
    );
}
