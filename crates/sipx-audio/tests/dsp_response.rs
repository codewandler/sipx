//! The measured magnitude response of the built-in filters, from outside the crate:
//! `docs/specs/call-dsp-effects.md` §10 (`X-109`).
//!
//! `M-65` shipped three filters and asserted their impulse and step behaviour at one cutoff, where
//! the shipped coefficient is exactly 0.5 and the section collapses to a two-tap average. What it
//! could not say is what any of them does to a tone at some other frequency — §8.4 states in so
//! many words that `band_gain` "is not the magnitude response at the centre frequency" and that
//! `X-109` owns the measurement that would let anyone claim otherwise. This is that measurement.
//!
//! **Two kinds of assertion live here and they are not interchangeable.** The `predicted_*` tests
//! state what the design says the sweep must produce and were written before the sweep was run:
//! unity for an identity, unity for a polarity inversion because magnitude is blind to phase, a
//! monotone roll-off for a one-pole low-pass, unity at every bin for a peaking filter at its
//! identity gain, and a peak strictly below the `band_gain` a caller sets. The `recorded_*` tests
//! carry integers taken from a run and locked, exactly as
//! `docs/specs/call-dsp-noise-reduction.md` §8 does: they are there so a change to the arithmetic
//! moves a number rather than a judgement, and no reader should read them as anything a hand
//! derived.
//!
//! Everything here is integer arithmetic over a shipped table, so a figure is the same on every
//! machine, under any load, and needs no quiet box to be worth recording.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_audio::dsp::effects::{Gain, HighPass, LowPass, Peaking, Polarity, Stutter};
use sipx_audio::dsp::noise::SubbandSuppressor;
use sipx_audio::dsp::response::{
    RESPONSE_BLOCK_POSITIONS, ResponseError, ResponseSweep, SWEEP_AMPLITUDE, SWEEP_BINS,
};
use sipx_audio::dsp::{FrameProcessor, LengthPolicy, Parameter, ParameterValue, StreamFormat};

// ------------------------------------------------------------------------- the driver ----

fn d8() -> StreamFormat {
    StreamFormat::new(8_000, 1).unwrap()
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

/// Configure a fresh processor from `parameters` and sweep it on `format`.
fn sweep<P, F>(format: StreamFormat, parameters: &[Parameter], mut build: F) -> Vec<u32>
where
    P: FrameProcessor,
    F: FnMut() -> P,
{
    ResponseSweep::new(format)
        .run(|| {
            let mut processor = build();
            processor.configure(parameters).unwrap();
            processor
        })
        .unwrap()
        .magnitudes()
        .collect()
}

// ------------------------------------------------------- predicted before the run ----

/// RESP-P1 — an identity processor measures unity at every bin, so the sweep does not invent a
/// roll-off that the processor did not apply.
#[test]
fn predicted_identity_is_unity_everywhere() {
    let measured = sweep(d8(), &[], Gain::new);
    assert_eq!(measured, vec![1_000; SWEEP_BINS.len()], "{measured:?}");
}

/// RESP-P2 — a polarity inversion measures unity at every bin. The sweep reports a *magnitude*,
/// and a measure that compared output to input sample by sample would report zero here.
#[test]
fn predicted_polarity_is_unity_everywhere() {
    let measured = sweep(d8(), &[flag("inverted", true)], Polarity::new);
    assert_eq!(measured, vec![1_000; SWEEP_BINS.len()], "{measured:?}");
}

/// RESP-P3 — a one-pole low-pass never rises with frequency and a one-pole high-pass never falls.
/// Monotonicity is the shape of `H(z) = G(1 + z⁻¹)/(1 + (2G − 1)z⁻¹)` and not a property of any
/// one run.
#[test]
fn predicted_one_pole_slopes_are_monotone() {
    let low = sweep(d8(), &[integer("cutoff_hz", 1_000)], LowPass::new);
    let high = sweep(d8(), &[integer("cutoff_hz", 1_000)], HighPass::new);
    for pair in low.windows(2) {
        assert!(pair[1] <= pair[0], "low-pass rose: {low:?}");
    }
    for pair in high.windows(2) {
        assert!(pair[1] >= pair[0], "high-pass fell: {high:?}");
    }
    assert!(low[0] > low[low.len() - 1], "low-pass is flat: {low:?}");
    assert!(
        high[0] < high[high.len() - 1],
        "high-pass is flat: {high:?}"
    );
}

/// RESP-P4 — `band_gain = 1.0` is the exact identity at every bin, which is §8.4's own claim.
#[test]
fn predicted_unity_peaking_is_flat() {
    let measured = sweep(d8(), &[ratio("band_gain", 1_000)], Peaking::new);
    assert_eq!(measured, vec![1_000; SWEEP_BINS.len()], "{measured:?}");
}

/// RESP-P5 — §8.4's "does not": the peak a `band_gain` produces is **smaller** than the number
/// suggests, because a first-order pair's extracted band does not reach unity. Predicted before
/// the run; the size of the shortfall is `recorded_peaking_band_gain_is_not_the_peak`'s.
#[test]
fn predicted_peaking_peak_is_below_its_band_gain() {
    for thousandths in [2_000, 4_000] {
        let measured = ResponseSweep::new(d8())
            .run(|| {
                let mut peaking = Peaking::new();
                peaking
                    .configure(&[integer("centre_hz", 1_000), ratio("band_gain", thousandths)])
                    .unwrap();
                peaking
            })
            .unwrap();
        let peak = measured.magnitudes().max().unwrap();
        assert!(
            peak < u32::try_from(thousandths).unwrap(),
            "band_gain {thousandths} reached {peak}",
        );
        assert!(
            peak > 1_000,
            "band_gain {thousandths} lifted nothing: {peak}"
        );
    }
}

/// RESP-P6 — the same tone in both channels measures the same response as one channel. A filter
/// carries per-channel state, so this is the assertion that it carries *separate* per-channel
/// state rather than one shared section.
#[test]
fn predicted_stereo_matches_mono() {
    let mono = sweep(d8(), &[integer("cutoff_hz", 1_000)], LowPass::new);
    let stereo = sweep(
        StreamFormat::new(8_000, 2).unwrap(),
        &[integer("cutoff_hz", 1_000)],
        LowPass::new,
    );
    assert_eq!(mono, stereo, "mono {mono:?} stereo {stereo:?}");
}

/// RESP-P7 — a bin is a fraction of the sample rate, so the same cutoff in hertz sits at a
/// different bin on a wideband call and the two sweeps must differ. A sweep that reported the
/// same numbers at 8,000 and 16,000 Hz would be reporting the table and not the filter.
#[test]
fn predicted_rate_moves_the_curve() {
    let narrow = sweep(d8(), &[integer("cutoff_hz", 1_000)], LowPass::new);
    let wide = sweep(
        StreamFormat::new(16_000, 1).unwrap(),
        &[integer("cutoff_hz", 1_000)],
        LowPass::new,
    );
    assert_ne!(narrow, wide, "the rate changed nothing: {narrow:?}");
    // Every bin is twice the frequency at twice the rate, so the wideband curve is below the
    // narrowband one everywhere but at the bin where both are already unity.
    for (n, w) in narrow.iter().zip(&wide) {
        assert!(
            w <= n,
            "wideband rose above narrowband: {narrow:?} {wide:?}"
        );
    }
}

// ------------------------------------------------------ what the sweep refuses to guess ----

/// RESP-P8 — a processor that delays its output is refused by name rather than swept anyway. The
/// measured block would be a settled tone offset by the latency, which is a phase the sweep does
/// not measure and cannot correct for.
#[test]
fn refuses_a_delaying_processor() {
    let error = ResponseSweep::new(d8())
        .run(|| Stutter::new(4).unwrap())
        .unwrap_err();
    assert_eq!(error, ResponseError::LatencyNotZero { positions: 4 });
    assert!(error.to_string().contains("latency"), "{error}");
}

/// RESP-P9 — a rate the processor refuses is reported as the processor's own typed refusal, not
/// as an absent row. `docs/specs/custom-call-dsp.md`'s rule that an unavailable case is named
/// rather than skipped is the whole reason this returns a `Result`.
#[test]
fn refuses_a_format_the_processor_refuses() {
    let format = StreamFormat::new(8_000, 4).unwrap();
    let error = ResponseSweep::new(format).run(LowPass::new).unwrap_err();
    assert!(
        matches!(error, ResponseError::Format(_)),
        "expected a format refusal, got {error}",
    );
}

/// RESP-P10 — a bin at or above half the block is not a tone this corpus can carry, and the sweep
/// says so instead of measuring the silence such a bin would generate.
#[test]
fn refuses_a_bin_it_cannot_carry() {
    let error = ResponseSweep::new(d8())
        .with_bins(&[128])
        .run(Gain::new)
        .unwrap_err();
    assert_eq!(error, ResponseError::BinOutOfRange { bin: 128 });
}

/// RESP-P11 — the corpus is the same integers for everybody: the block is 256 positions, the
/// amplitude is half of half scale so a lift of up to 4.0 cannot clip, and the bins are the eight
/// §10 records.
#[test]
fn the_corpus_is_fixed() {
    assert_eq!(RESPONSE_BLOCK_POSITIONS, 256);
    assert_eq!(SWEEP_AMPLITUDE, 8_192);
    assert_eq!(SWEEP_BINS, &[2, 4, 8, 16, 32, 64, 96, 112]);
    // Every built-in the sweep accepts is length-preserving; that is what makes the measured block
    // line up with the block that produced it.
    assert_eq!(
        LowPass::new().capability().length(),
        LengthPolicy::Preserving
    );
}

// ------------------------------------------------- recorded from a run, and then locked ----

/// RESP-R1 — the low-pass at three cutoffs, and `docs/specs/call-dsp-effects.md` §10's table.
#[test]
fn recorded_low_pass() {
    assert_eq!(
        sweep(d8(), &[integer("cutoff_hz", 300)], LowPass::new),
        vec![979, 923, 768, 511, 275, 117, 49, 24],
    );
    assert_eq!(
        sweep(d8(), &[integer("cutoff_hz", 1_000)], LowPass::new),
        vec![998, 993, 973, 901, 707, 383, 169, 82],
    );
    assert_eq!(
        sweep(d8(), &[integer("cutoff_hz", 3_400)], LowPass::new),
        vec![1_000, 1_000, 1_000, 999, 995, 972, 865, 638],
    );
}

/// RESP-R2 — the high-pass at the same three cutoffs.
#[test]
fn recorded_high_pass() {
    assert_eq!(
        sweep(d8(), &[integer("cutoff_hz", 300)], HighPass::new),
        vec![203, 384, 640, 860, 962, 993, 999, 1_000],
    );
    assert_eq!(
        sweep(d8(), &[integer("cutoff_hz", 1_000)], HighPass::new),
        vec![59, 118, 231, 433, 707, 924, 986, 997],
    );
    assert_eq!(
        sweep(d8(), &[integer("cutoff_hz", 3_400)], HighPass::new),
        vec![6, 12, 24, 48, 99, 233, 501, 770],
    );
}

/// RESP-R3 — **`cutoff_hz` is the half-power point, to the thousandth, at every cutoff the sweep
/// can land a bin on.** This was not predicted: §8.2 says the coefficient comes from a 65-entry
/// table with integer interpolation and says nothing about where the resulting corner lands, so
/// until this run "cutoff" was a parameter name. It is 707 thousandths — `⌊1000/√2⌉` — for the
/// low-pass and the high-pass alike, at five cutoffs two octaves apart, which is the table and the
/// interpolation being right rather than one value being lucky.
#[test]
fn recorded_cutoff_is_the_half_power_point() {
    for hertz in [125_i64, 250, 500, 1_000, 2_000] {
        let bin = u16::try_from(hertz * 256 / 8_000).unwrap();
        let low = ResponseSweep::new(d8())
            .with_bins(&[bin])
            .run(|| {
                let mut filter = LowPass::new();
                filter.configure(&[integer("cutoff_hz", hertz)]).unwrap();
                filter
            })
            .unwrap();
        let high = ResponseSweep::new(d8())
            .with_bins(&[bin])
            .run(|| {
                let mut filter = HighPass::new();
                filter.configure(&[integer("cutoff_hz", hertz)]).unwrap();
                filter
            })
            .unwrap();
        assert_eq!(low.magnitude_at(bin), Some(707), "low-pass at {hertz} Hz");
        assert_eq!(high.magnitude_at(bin), Some(707), "high-pass at {hertz} Hz");
    }
}

/// RESP-R4 — the figure §8.4 owed. A `band_gain` of 4.0 measures **3.001** at its own centre
/// frequency, and one of 2.0 measures 1.667: the peak is a quarter and a sixth below the parameter
/// respectively, because a first-order pair's extracted band does not reach unity.
#[test]
fn recorded_peaking_band_gain_is_not_the_peak() {
    let at = |thousandths: i32| {
        sweep(
            d8(),
            &[integer("centre_hz", 1_000), ratio("band_gain", thousandths)],
            Peaking::new,
        )
    };
    assert_eq!(at(0), vec![990, 963, 866, 620, 336, 620, 902, 975]);
    assert_eq!(at(500), vec![994, 977, 919, 784, 667, 784, 940, 985]);
    assert_eq!(at(1_000), vec![1_000; 8]);
    assert_eq!(
        at(2_000),
        vec![1_019, 1_071, 1_225, 1_494, 1_667, 1_494, 1_172, 1_048]
    );
    assert_eq!(
        at(4_000),
        vec![1_083, 1_289, 1_803, 2_559, 3_001, 2_559, 1_637, 1_199]
    );
    // The extracted band reaches 667 thousandths of unity and no more, and above unity that one
    // number is the whole of the shortfall: `1000 + (band_gain − 1000)·667/1000` is the measured
    // peak to the thousandth at 2.0 and at 4.0.
    for thousandths in [2_000_i64, 4_000] {
        let modelled = 1_000 + (thousandths - 1_000) * 667 / 1_000;
        let measured = i64::from(at(i32::try_from(thousandths).unwrap())[4]);
        assert!(
            measured.abs_diff(modelled) <= 1,
            "band_gain {thousandths}: measured {measured}, band model says {modelled}",
        );
    }
    // **Below unity the same model under-reads, and by more than rounding.** A cut of the band is
    // a subtraction of something not exactly in phase with what it is subtracted from, so it
    // cancels slightly less than a scalar model expects: at `band_gain = 0` the model says 333 and
    // the filter measures 336. Recorded rather than smoothed over — a caller sizing a notch gets
    // less notch than the arithmetic suggests, on top of the shortfall the peak already has.
    assert_eq!(at(0)[4], 336);
    assert_eq!(1_000 + (0 - 1_000) * 667 / 1_000, 333);
}

/// RESP-R5 — the same filters on a wideband call. Every bin is twice the frequency, so the same
/// hertz sits an octave lower on the curve.
#[test]
fn recorded_wideband() {
    let wideband = StreamFormat::new(16_000, 1).unwrap();
    assert_eq!(
        sweep(wideband, &[integer("cutoff_hz", 3_400)], LowPass::new),
        vec![1_000, 998, 992, 970, 885, 619, 310, 155],
    );
    assert_eq!(
        sweep(wideband, &[integer("cutoff_hz", 300)], HighPass::new),
        vec![385, 641, 859, 959, 990, 998, 1_000, 1_000],
    );
}

/// RESP-R6 — a pure gain measures exactly itself at every bin, in both directions. This is the
/// sweep's own calibration: a scale factor is the one response whose answer is known in advance,
/// and it comes back exact rather than within a thousandth.
#[test]
fn recorded_pure_gain_is_exact() {
    assert_eq!(sweep(d8(), &[ratio("gain", 500)], Gain::new), vec![500; 8]);
    assert_eq!(
        sweep(d8(), &[ratio("gain", 2_000)], Gain::new),
        vec![2_000; 8]
    );
}

/// RESP-R7 — **the sweep sees a nonlinearity, and this is the evidence that it can.** A linear
/// processor measures the same at every probe amplitude; the peaking filter at `band_gain = 4.0`
/// does, at a quarter and at an eighth of full scale — and stops doing so at a half, where its own
/// lift drives the output into the clamp. The three bins around the centre are the ones that move.
///
/// This is also why [`SWEEP_AMPLITUDE`] is what it is: a sweep run at half scale would have
/// recorded the clamp in §10's table and called it a filter.
#[test]
fn recorded_amplitude_reveals_the_clamp() {
    let at = |amplitude: i16| {
        ResponseSweep::new(d8())
            .with_amplitude(amplitude)
            .run(|| {
                let mut peaking = Peaking::new();
                peaking
                    .configure(&[integer("centre_hz", 1_000), ratio("band_gain", 4_000)])
                    .unwrap();
                peaking
            })
            .unwrap()
            .magnitudes()
            .collect::<Vec<_>>()
    };
    let eighth = at(1_024);
    let quarter = at(SWEEP_AMPLITUDE);
    let half = at(16_384);
    assert_eq!(
        quarter,
        vec![1_083, 1_289, 1_803, 2_559, 3_001, 2_559, 1_637, 1_199]
    );
    // Linear to within the probe's own rounding at an eighth and a quarter of full scale.
    for (small, large) in eighth.iter().zip(&quarter) {
        assert!(
            small.abs_diff(*large) <= 1,
            "{eighth:?} against {quarter:?}"
        );
    }
    // Not linear at a half: the clamp takes 300 to 550 thousandths off the lifted bins.
    assert_eq!(
        half,
        vec![1_083, 1_289, 1_803, 2_262, 2_451, 2_204, 1_637, 1_199]
    );
    assert!(
        half[4] + 500 < quarter[4],
        "the clamp did not show: {half:?}"
    );
}

/// RESP-R8 — the noise reducer under a steady tone, which is **not** a transfer function and is
/// recorded because what it shows is worth knowing: after its 1,024-position warm-up the
/// `sipx.subband_suppressor` leaves a settled tone essentially untouched — 4 thousandths off at
/// worst after 2,048 positions, 17 after 8,192.
///
/// That is the recursive-minimum floor of `docs/specs/call-dsp-noise-reduction.md` §5.2 doing what
/// §5.6 says it does and nothing more: a tone raises the envelope as fast as it raises the floor,
/// so the ratio between them barely moves and the band gain stays near unity. It is **not** a
/// figure about noise reduction — §8's corpus is, and it is broadband noise rather than a tone —
/// and it must not be read as one. What it rules out is the opposite failure: a reducer that
/// gated a steady talker would show up here as a collapse toward `min_band_gain`, and does not.
#[test]
fn recorded_noise_reducer_leaves_a_tone_alone() {
    let settled = |blocks: u32| {
        ResponseSweep::new(d8())
            .with_settle_blocks(blocks)
            .run(SubbandSuppressor::new)
            .unwrap()
            .magnitudes()
            .collect::<Vec<_>>()
    };
    // Inside the warm-up the reducer is the exact identity (§5.4), and the sweep shows it.
    assert_eq!(settled(3), vec![1_000; 8]);
    assert_eq!(settled(8), vec![999, 998, 997, 996, 996, 997, 997, 998]);
    assert_eq!(settled(32), vec![995, 992, 988, 984, 983, 985, 987, 990]);
}
