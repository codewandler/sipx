//! G.711 µ-law and A-law (ITU-T G.711).
//!
//! Two logarithmic 8-bit codecs, both ancient and both still the only thing every endpoint on
//! earth agrees on. The compression is a piecewise-linear approximation of a logarithm: a sign
//! bit, a 3-bit exponent and a 4-bit mantissa.
//!
//! What makes these worth testing carefully rather than round-tripping: the round trip is
//! *lossy by design*, so "encode then decode gives back the input" is false for almost every
//! input and cannot be the test. The reference values below come from the ITU algorithm, and
//! what the round trip does guarantee is that a decoded value re-encodes to the same code —
//! the codec is idempotent on its own output, which is the property that matters when audio
//! passes through more than one hop.

const ULAW_BIAS: i32 = 0x84;
const ULAW_CLIP: i32 = 32_635;
const ALAW_CLIP: i32 = 32_635;

/// Encode one sample to µ-law.
#[must_use]
pub fn ulaw_encode(sample: i16) -> u8 {
    let mut pcm = i32::from(sample);
    let sign = if pcm < 0 { 0x80 } else { 0x00 };
    if pcm < 0 {
        pcm = -pcm;
    }
    pcm = pcm.min(ULAW_CLIP);
    pcm += ULAW_BIAS;

    let mut exponent = 7i32;
    let mut mask = 0x4000i32;
    while exponent > 0 && (pcm & mask) == 0 {
        exponent -= 1;
        mask >>= 1;
    }
    let mantissa = (pcm >> (exponent + 3)) & 0x0F;
    let code = sign | (exponent << 4) | mantissa;
    // The complement is not decoration: it puts the most common values (near silence) at codes
    // with many 1 bits, which survive a lost bit better on the analogue lines this was
    // designed for.
    u8::try_from(!code & 0xFF).unwrap_or(0)
}

/// Decode one µ-law code.
#[must_use]
pub fn ulaw_decode(code: u8) -> i16 {
    let code = i32::from(!code);
    let sign = code & 0x80;
    let exponent = (code >> 4) & 0x07;
    let mantissa = code & 0x0F;
    let mut sample = ((mantissa << 3) + ULAW_BIAS) << exponent;
    sample -= ULAW_BIAS;
    let sample = if sign != 0 { -sample } else { sample };
    i16::try_from(sample.clamp(i32::from(i16::MIN), i32::from(i16::MAX))).unwrap_or(0)
}

/// Encode one sample to A-law.
#[must_use]
pub fn alaw_encode(sample: i16) -> u8 {
    let mut pcm = i32::from(sample);
    // A-law's sign convention is the opposite of µ-law's, and the negative branch subtracts
    // one. Both are easy to "fix" into something that sounds almost right and is wrong.
    let sign = if pcm >= 0 { 0x80 } else { 0x00 };
    if pcm < 0 {
        pcm = -pcm - 1;
    }
    pcm = pcm.min(ALAW_CLIP);

    let code = if pcm < 256 {
        pcm >> 4
    } else {
        let mut exponent = 7i32;
        let mut mask = 0x4000i32;
        while exponent > 0 && (pcm & mask) == 0 {
            exponent -= 1;
            mask >>= 1;
        }
        let mantissa = (pcm >> (exponent + 3)) & 0x0F;
        (exponent << 4) | mantissa
    };
    // The 0x55 toggle spreads the alternating bit pattern that keeps a line's clock recovery
    // happy during silence.
    u8::try_from((code ^ sign ^ 0x55) & 0xFF).unwrap_or(0)
}

/// Decode one A-law code.
#[must_use]
pub fn alaw_decode(code: u8) -> i16 {
    let code = i32::from(code ^ 0x55);
    let sign = code & 0x80;
    let exponent = (code >> 4) & 0x07;
    let mantissa = code & 0x0F;
    let sample = if exponent == 0 {
        (mantissa << 4) + 8
    } else {
        ((mantissa << 4) + 0x108) << (exponent - 1)
    };
    let sample = if sign != 0 { sample } else { -sample };
    i16::try_from(sample.clamp(i32::from(i16::MIN), i32::from(i16::MAX))).unwrap_or(0)
}

/// Encode a buffer of samples to µ-law.
#[must_use]
pub fn ulaw_encode_all(samples: &[i16]) -> Vec<u8> {
    samples.iter().copied().map(ulaw_encode).collect()
}

/// Decode a buffer of µ-law.
#[must_use]
pub fn ulaw_decode_all(codes: &[u8]) -> Vec<i16> {
    codes.iter().copied().map(ulaw_decode).collect()
}

/// Encode a buffer of samples to A-law.
#[must_use]
pub fn alaw_encode_all(samples: &[i16]) -> Vec<u8> {
    samples.iter().copied().map(alaw_encode).collect()
}

/// Decode a buffer of A-law.
#[must_use]
pub fn alaw_decode_all(codes: &[u8]) -> Vec<i16> {
    codes.iter().copied().map(alaw_decode).collect()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// Values from the ITU-T G.711 algorithm, computed independently of this implementation.
    /// A codec checked only by round-tripping proves its two halves agree with each other,
    /// not that either is right — and two halves that are wrong in mirrored ways round-trip
    /// perfectly while interoperating with nothing.
    #[test]
    fn ulaw_matches_the_itu_reference_table() {
        const REFERENCE: &[(i16, u8)] = &[
            (0, 255),
            (1, 255),
            (-1, 127),
            (100, 242),
            (-100, 114),
            (1000, 206),
            (-1000, 78),
            (8000, 160),
            (-8000, 32),
            (32767, 128),
            (-32768, 0),
            (4096, 175),
            (-4096, 47),
        ];
        for &(sample, expected) in REFERENCE {
            assert_eq!(ulaw_encode(sample), expected, "µ-law encoding of {sample}");
        }
    }

    #[test]
    fn alaw_matches_the_itu_reference_table() {
        const REFERENCE: &[(i16, u8)] = &[
            (0, 213),
            (1, 213),
            (-1, 85),
            (100, 211),
            (-100, 83),
            (1000, 250),
            (-1000, 122),
            (8000, 138),
            (-8000, 10),
            (32767, 170),
            (-32768, 42),
            (4096, 133),
            (-4096, 26),
        ];
        for &(sample, expected) in REFERENCE {
            assert_eq!(alaw_encode(sample), expected, "A-law encoding of {sample}");
        }
    }

    #[test]
    fn ulaw_decoding_matches_the_reference() {
        assert_eq!(ulaw_decode(0xFF), 0);
        assert_eq!(ulaw_decode(0x7F), 0);
        assert_eq!(ulaw_decode(0x00), -32_124);
        assert_eq!(ulaw_decode(0x80), 32_124);
    }

    #[test]
    fn alaw_decoding_matches_the_reference() {
        assert_eq!(alaw_decode(0xD5), 8);
        assert_eq!(alaw_decode(0x55), -8);
        assert_eq!(alaw_decode(0x2A), -32_256);
        assert_eq!(alaw_decode(0xAA), 32_256);
    }

    /// Both codecs are symmetric about zero: the codes as a whole sum to nothing. A sign-handling
    /// bug on one side shows up here even when the individual values look plausible.
    #[test]
    fn the_codec_is_symmetric_about_zero() {
        let ulaw_sum: i64 = (0..=255u8).map(|c| i64::from(ulaw_decode(c))).sum();
        let alaw_sum: i64 = (0..=255u8).map(|c| i64::from(alaw_decode(c))).sum();
        assert_eq!(ulaw_sum, 0, "µ-law is not symmetric");
        assert_eq!(alaw_sum, 0, "A-law is not symmetric");
    }

    /// The round trip is lossy by design, so this is the property that actually holds: a
    /// decoded value re-encodes to the code it came from. Without it, audio passing through
    /// two hops would degrade at every one.
    ///
    /// With exactly one exception, which is a property of µ-law rather than of this code.
    /// µ-law has two representations of zero — code 255 is +0 and code 127 is −0 — and both
    /// decode to the same sample, so the encoder has to pick one. Every other code, in both
    /// codecs, is idempotent; A-law has no such pair.
    #[test]
    fn decoding_then_encoding_returns_the_same_code() {
        const ULAW_NEGATIVE_ZERO: u8 = 127;

        for code in 0..=255u8 {
            if code == ULAW_NEGATIVE_ZERO {
                assert_eq!(ulaw_decode(code), 0, "code 127 is µ-law's negative zero");
                assert_eq!(
                    ulaw_encode(ulaw_decode(code)),
                    255,
                    "and it normalises to positive zero"
                );
            } else {
                assert_eq!(
                    ulaw_encode(ulaw_decode(code)),
                    code,
                    "µ-law is not idempotent at code {code}"
                );
            }

            assert_eq!(
                alaw_encode(alaw_decode(code)),
                code,
                "A-law is not idempotent at code {code}"
            );
        }
    }

    /// Quantisation error must stay within the codec's step size. A wrong exponent produces
    /// values that are close for small samples and wildly off for large ones, which this
    /// catches and a spot check would not.
    #[test]
    fn quantisation_error_stays_within_the_step_size() {
        for sample in (-32_768..=32_767).step_by(37) {
            let sample = i16::try_from(sample).expect("in range");
            let error = (i32::from(ulaw_decode(ulaw_encode(sample))) - i32::from(sample)).abs();
            let allowed = (i32::from(sample).abs() >> 5).max(16);
            assert!(
                error <= allowed,
                "µ-law error {error} at sample {sample} exceeds {allowed}"
            );
        }
    }

    /// Values beyond what the codec can represent must clip, not wrap. Wrapping turns a loud
    /// sound into a loud sound of the opposite sign, which is heard as a click.
    #[test]
    fn loud_samples_clip_rather_than_wrapping() {
        assert_eq!(ulaw_encode(32_767), ulaw_encode(32_000));
        assert!(ulaw_decode(ulaw_encode(32_767)) > 30_000);
        assert!(ulaw_decode(ulaw_encode(-32_768)) < -30_000);
        assert!(alaw_decode(alaw_encode(32_767)) > 30_000);
        assert!(alaw_decode(alaw_encode(-32_768)) < -30_000);
    }

    #[test]
    fn buffers_encode_and_decode_as_wholes() {
        let samples: Vec<i16> = (0..160).map(|i| i * 100 - 8000).collect();
        let encoded = ulaw_encode_all(&samples);
        assert_eq!(encoded.len(), samples.len());
        let decoded = ulaw_decode_all(&encoded);
        assert_eq!(decoded.len(), samples.len());
        assert_eq!(
            ulaw_encode_all(&decoded),
            encoded,
            "idempotent over a buffer"
        );
    }
}
