//! The entropy pool and its derivation tape (`docs/specs/browser-sdk.md` §4.7, §8.4).
//!
//! Randomness is a host input, never an ambient capability. There is no time seed, no counter
//! seed, no constant and no weaker generator anywhere in this module: an operation that needs
//! more entropy than the pool holds fails whole with `E_ENTROPY`, having consumed nothing.
//!
//! That is also why the browser build drops `sipx-sdp`'s `sdes-keys` and `sipx-sip`'s `identity`
//! features. Neither is wanted here, and both reach an operating-system entropy source — which
//! on this target would have to be imported from JavaScript, and §4.1 says the module imports
//! nothing.
//!
//! The pool is an **ordered tape**: derivations take octets from the front, in the order the
//! identifiers are first required, so a pinned tape yields pinned identifiers. `BSDK-ENT-1` is
//! that property written down.

use std::collections::VecDeque;

use crate::bounds;
use crate::error::Error;

/// Octets consumed by each derived identifier (§4.7's table).
const CALL_ID_OCTETS: usize = 16;
const TAG_OCTETS: usize = 8;
const BRANCH_OCTETS: usize = 8;
const CNONCE_OCTETS: usize = 16;

/// RFC 3261 §8.1.1.7's magic cookie, which every branch this kernel mints carries.
const BRANCH_MAGIC_COOKIE: &str = "z9hG4bK";

/// A bounded, host-fed pool of `crypto.getRandomValues` octets.
#[derive(Debug, Default)]
pub(crate) struct Pool {
    tape: VecDeque<u8>,
}

impl Pool {
    /// How many octets remain.
    pub(crate) fn level(&self) -> usize {
        self.tape.len()
    }

    /// Whether the pool has fallen below the low-water mark and the host should be asked for
    /// more.
    pub(crate) fn below_low_water(&self) -> bool {
        self.tape.len() < bounds::ENTROPY_LOW_WATER
    }

    /// Append host entropy.
    ///
    /// Feeding beyond capacity is `E_BOUNDS` **with the pool unchanged** — a partial accept
    /// would leave the host believing it had delivered bytes the kernel silently dropped.
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self.tape.len().saturating_add(bytes.len()) > bounds::ENTROPY_CAPACITY {
            return Err(Error::Bounds);
        }
        self.tape.extend(bytes.iter().copied());
        Ok(())
    }

    /// Take `count` octets atomically from the front of the tape.
    ///
    /// Either the whole draw succeeds or nothing is consumed: a half-drawn identifier is a
    /// predictable identifier for the half that was not drawn.
    fn take(&mut self, count: usize) -> Result<Vec<u8>, Error> {
        if self.tape.len() < count {
            return Err(Error::Entropy);
        }
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            // The length was checked above; `Error::Entropy` here would mean the deque
            // disagreed with its own `len`, which is a defect rather than an empty pool.
            out.push(self.tape.pop_front().ok_or(Error::Entropy)?);
        }
        Ok(out)
    }

    /// A Call-ID: 16 octets as 32 lowercase hex characters, with no `@host` part.
    pub(crate) fn call_id(&mut self) -> Result<String, Error> {
        Ok(hex(&self.take(CALL_ID_OCTETS)?))
    }

    /// A From/To tag: 8 octets as 16 lowercase hex characters.
    pub(crate) fn tag(&mut self) -> Result<String, Error> {
        Ok(hex(&self.take(TAG_OCTETS)?))
    }

    /// A Via branch: the magic cookie followed by 8 octets as 16 lowercase hex characters.
    pub(crate) fn branch(&mut self) -> Result<String, Error> {
        Ok(format!(
            "{BRANCH_MAGIC_COOKIE}{}",
            hex(&self.take(BRANCH_OCTETS)?)
        ))
    }

    /// A digest cnonce: 16 octets as 32 lowercase hex characters.
    pub(crate) fn cnonce(&mut self) -> Result<String, Error> {
        Ok(hex(&self.take(CNONCE_OCTETS)?))
    }

    /// Zeroise the tape.
    ///
    /// Hygiene on `sipx_kernel_free`, and documented in §8.3 as *not* a confidentiality
    /// boundary: any script in the origin can read linear memory while the kernel is alive.
    pub(crate) fn zeroise(&mut self) {
        for octet in &mut self.tape {
            *octet = 0;
        }
        self.tape.clear();
    }
}

/// Lowercase hex digits. A table lookup rather than `write!`, because formatting to a `String`
/// nonetheless returns a `fmt::Error`, and swallowing it would be the one place in this crate
/// that discarded an error silently.
const DIGITS: [u8; 16] = *b"0123456789abcdef";

/// Lowercase hex, which is what every identifier in §4.7's table is rendered as.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let hi = usize::from(byte >> 4);
        let lo = usize::from(byte & 0x0f);
        out.push(char::from(DIGITS.get(hi).copied().unwrap_or(b'0')));
        out.push(char::from(DIGITS.get(lo).copied().unwrap_or(b'0')));
    }
    out
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

    /// The 32-octet tape `00 01 02 … 1f` from `BSDK-ENT-1`.
    fn ent_1_tape() -> Vec<u8> {
        (0u8..32).collect()
    }

    #[test]
    fn bsdk_ent_1_derives_the_pinned_identifiers_in_order() {
        let mut pool = Pool::default();
        pool.feed(&ent_1_tape()).expect("within capacity");

        assert_eq!(pool.call_id().unwrap(), "000102030405060708090a0b0c0d0e0f");
        assert_eq!(pool.tag().unwrap(), "1011121314151617");
        assert_eq!(pool.branch().unwrap(), "z9hG4bK18191a1b1c1d1e1f");
        assert_eq!(pool.level(), 0);
    }

    #[test]
    fn an_exhausted_pool_consumes_nothing() {
        let mut pool = Pool::default();
        pool.feed(&[0xaa; 4]).expect("within capacity");
        assert_eq!(pool.tag(), Err(Error::Entropy));
        // "no partial consumption": the four octets are still there.
        assert_eq!(pool.level(), 4);
    }

    #[test]
    fn feeding_beyond_capacity_leaves_the_pool_unchanged() {
        let mut pool = Pool::default();
        pool.feed(&[0x01; 1000]).expect("within capacity");
        assert_eq!(pool.feed(&[0x02; 25]), Err(Error::Bounds));
        assert_eq!(pool.level(), 1000);
    }

    #[test]
    fn low_water_is_sixty_four() {
        let mut pool = Pool::default();
        assert!(pool.below_low_water());
        pool.feed(&[0u8; 64]).expect("within capacity");
        assert!(!pool.below_low_water());
        let _ = pool.tag().unwrap();
        assert!(pool.below_low_water());
    }

    #[test]
    fn zeroise_empties_the_tape() {
        let mut pool = Pool::default();
        pool.feed(&[0xff; 32]).expect("within capacity");
        pool.zeroise();
        assert_eq!(pool.level(), 0);
    }
}
