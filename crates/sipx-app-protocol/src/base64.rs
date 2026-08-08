//! Base64 (RFC 4648 §4), for `play.source.inline` and nothing else.
//!
//! Forty lines rather than a dependency: §6.5 of the contract admits exactly one binary field, the
//! standard alphabet with padding, and a decoder for it is smaller than the argument for taking a
//! crate. It is strict — no whitespace, no alternative alphabet, no missing padding — because the
//! one thing an audio payload must not do is decode differently on two hosts.

/// The standard alphabet, in value order.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode bytes, with padding.
pub(crate) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let (b0, b1, b2) = (
            u32::from(chunk.first().copied().unwrap_or(0)),
            u32::from(chunk.get(1).copied().unwrap_or(0)),
            u32::from(chunk.get(2).copied().unwrap_or(0)),
        );
        let triple = (b0 << 16) | (b1 << 8) | b2;
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                let index = ((triple >> shift) & 0x3f) as usize;
                out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decode, strictly. `None` for anything that is not exactly what [`encode`] writes.
pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let mut quad = 0u32;
        let mut padding = 0usize;
        for (i, byte) in chunk.iter().enumerate() {
            let value = if *byte == b'=' {
                // Padding is only ever the last one or two characters of the last quad.
                if i < 2 {
                    return None;
                }
                padding += 1;
                0
            } else {
                if padding > 0 {
                    return None;
                }
                u32::try_from(ALPHABET.iter().position(|c| c == byte)?).ok()?
            };
            quad = (quad << 6) | value;
        }
        let triple = quad.to_be_bytes();
        // `to_be_bytes` gives four bytes; the top one is always zero because a quad is 24 bits.
        for byte in triple.iter().skip(1).take(3 - padding) {
            out.push(*byte);
        }
    }
    Some(out)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// RFC 4648 §10's own test vectors.
    #[test]
    fn the_rfc_s_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(plain.as_bytes()), encoded, "encoding {plain:?}");
            assert_eq!(
                decode(encoded).as_deref(),
                Some(plain.as_bytes()),
                "decoding {encoded:?}"
            );
        }
    }

    #[test]
    fn every_byte_value_survives_a_round_trip() {
        for length in 0..=32usize {
            let bytes: Vec<u8> = (0..length)
                .map(|i| u8::try_from(i * 7 % 256).unwrap_or(0))
                .collect();
            assert_eq!(decode(&encode(&bytes)).as_deref(), Some(bytes.as_slice()));
        }
    }

    #[test]
    fn what_is_not_strict_base64_is_refused_rather_than_repaired() {
        for text in [
            "Zg", "Zg=", "Zg===", "Z g==", "Zm9v!", "=Zm8", "Z=m8", "Zm-v",
        ] {
            assert_eq!(decode(text), None, "{text} should be refused");
        }
    }
}
