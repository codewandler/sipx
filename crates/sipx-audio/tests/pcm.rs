//! Linear PCM format and resampling vectors (`M-43`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sipx_audio::{LinearResampler, Pcm, PcmEncoding, PcmFormat, PcmSamples};

/// M-43 / `linear-pcm.md` PCM-1: depth is explicit, so unsigned bytes cannot be interpreted as
/// signed 16-bit words. The exact extrema pin clipping and the unsigned midpoint.
#[test]
fn unsigned_eight_bit_pcm_converts_without_depth_assumptions() {
    let pcm = Pcm::new(
        PcmFormat::new(8_000, PcmEncoding::Unsigned8).expect("format"),
        PcmSamples::Unsigned8(vec![0, 128, 255]),
    )
    .expect("matching samples");
    assert_eq!(pcm.to_i16(8_000).expect("converts"), [-32_768, 0, 32_512]);
}

/// M-43 / `linear-pcm.md` PCM-2 and PCM-3: the rate conversion preserves exact source points and
/// streaming packet boundaries do not change the result.
#[test]
fn linear_resampling_is_rate_correct_and_stream_continuous() {
    let source = [-20_000, -12_000, -4_000, 4_000, 12_000, 20_000];
    let mut down = LinearResampler::new(16_000, 8_000).expect("rates");
    assert_eq!(down.push_i16(&source), [-20_000, -4_000, 12_000]);

    let mut whole = LinearResampler::new(8_000, 16_000).expect("rates");
    let together = whole.push_i16(&source);
    let mut chunked = LinearResampler::new(8_000, 16_000).expect("rates");
    let mut apart = chunked.push_i16(&source[..3]);
    apart.extend(chunked.push_i16(&source[3..]));
    assert_eq!(apart, together);
}

/// M-43 / `linear-pcm.md` PCM-4: an untrusted rate cannot size an allocation.
#[test]
fn unsupported_sample_rates_are_typed_refusals() {
    assert!(PcmFormat::new(0, PcmEncoding::Signed16).is_err());
    assert!(PcmFormat::new(384_001, PcmEncoding::Signed16).is_err());
}
