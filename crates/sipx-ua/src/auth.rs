//! Shared HTTP Digest authentication for SIP.
//!
//! Parsing, challenge selection and digest arithmetic live in the sans-I/O SIP core so
//! registration and call setup use one implementation. This module preserves the `sipx-ua` API
//! and supplies entropy at the runtime-facing layer.

pub use sipx_sip::auth::{
    Algorithm, Challenge, Credentials, respond, strongest, topmost_supported,
};

/// A fresh client nonce.
#[must_use]
pub fn new_cnonce() -> String {
    use rand::Rng as _;
    let value: u64 = rand::rng().random();
    format!("{value:016x}")
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    /// Two cnonces in a row must differ; a fixed one defeats the point of a client nonce.
    #[test]
    fn client_nonces_are_not_repeated() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            assert!(seen.insert(super::new_cnonce()), "a cnonce was repeated");
        }
    }
}
