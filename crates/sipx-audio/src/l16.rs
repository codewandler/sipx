//! L16 linear 16-bit RTP audio (RFC 3551 §4.5.11).

/// Encode signed samples in RTP's network byte order.
#[must_use]
pub fn encode(samples: &[i16]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(samples.len().saturating_mul(2));
    for sample in samples {
        encoded.extend_from_slice(&sample.to_be_bytes());
    }
    encoded
}

/// Decode complete signed network-order samples.
///
/// # Errors
///
/// Returns [`L16Error::OddLength`] when a trailing byte cannot form a sample.
pub fn decode(payload: &[u8]) -> Result<Vec<i16>, L16Error> {
    if !payload.len().is_multiple_of(2) {
        return Err(L16Error::OddLength(payload.len()));
    }
    Ok(payload
        .chunks_exact(2)
        .map(|word| i16::from_be_bytes(word.try_into().unwrap_or_default()))
        .collect())
}

/// A malformed L16 payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum L16Error {
    /// A signed 16-bit word is incomplete.
    #[error("L16 payload has odd length {0}")]
    OddLength(usize),
}
