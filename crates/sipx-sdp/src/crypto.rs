//! SDES: keying SRTP through SDP (RFC 4568).
//!
//! The mechanism is blunt. `a=crypto` carries the master key **in the SDP body**, base64-encoded
//! and otherwise in the clear, and whoever can read the signalling can decrypt the media. RFC
//! 4568 §7.1 is explicit that it therefore requires a secure signalling path, and treats that as
//! a condition of use rather than as advice.
//!
//! sipx enforces it rather than documenting it. [`Crypto::offer`] takes a flag saying whether the
//! signalling is secure and returns nothing when it is not, so an offer over cleartext SIP cannot
//! carry a key by forgetting a check somewhere. That is the difference between a stack that has
//! a rule and one that has a comment.
//!
//! What SDES cannot do is protect a key from an intermediary that terminates the TLS — a proxy,
//! a session border controller. For that the keying has to happen on the media path, which is
//! what DTLS-SRTP (RFC 5764) is for.

use std::fmt;

/// The crypto suite. Only the default SRTP transform is offered.
///
/// Deliberately not an open enum of every suite in the registry. sipx implements one transform;
/// listing suites it cannot perform would produce an offer it could not honour, which is worse
/// than a short list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suite {
    /// AES-128 counter mode with an 80-bit HMAC-SHA1 tag.
    AesCm128HmacSha1_80,
}

impl Suite {
    /// The token as it appears in SDP.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AesCm128HmacSha1_80 => "AES_CM_128_HMAC_SHA1_80",
        }
    }

    /// The suite a token names, if it is one sipx can perform.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        // Case-sensitive: RFC 4568 §9.2 defines these as tokens with fixed spelling, and a peer
        // that sends a different case is not offering this suite.
        (token == Self::AesCm128HmacSha1_80.as_str()).then_some(Self::AesCm128HmacSha1_80)
    }

    /// How many octets of master key and master salt it uses.
    #[must_use]
    pub fn key_and_salt_len(self) -> (usize, usize) {
        match self {
            Self::AesCm128HmacSha1_80 => (16, 14),
        }
    }
}

/// One `a=crypto` line.
#[derive(Clone, PartialEq, Eq)]
pub struct Crypto {
    /// The tag that identifies this offer among several.
    pub tag: u32,
    /// The transform.
    pub suite: Suite,
    /// Master key followed by master salt, concatenated as RFC 4568 §6.1 requires.
    pub key_and_salt: Vec<u8>,
}

impl fmt::Debug for Crypto {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The key. A derived `Debug` would put it in whatever log the caller writes, which for a
        // key carried in signalling is the likeliest way it escapes.
        f.debug_struct("Crypto")
            .field("tag", &self.tag)
            .field("suite", &self.suite)
            .finish_non_exhaustive()
    }
}

impl Crypto {
    /// A fresh offer, **only over a secure signalling path**.
    ///
    /// `None` when the signalling is not secure, and that is the point of the signature: RFC
    /// 4568 §7.1 makes a secure path a condition of use, and a function that returned a key
    /// regardless would leave every caller one forgotten check away from publishing it.
    #[must_use]
    pub fn offer(tag: u32, suite: Suite, secure_signalling: bool) -> Option<Self> {
        if !secure_signalling {
            return None;
        }
        let (key_len, salt_len) = suite.key_and_salt_len();
        let mut key_and_salt = vec![0u8; key_len + salt_len];
        fill_random(&mut key_and_salt);
        Some(Self {
            tag,
            suite,
            key_and_salt,
        })
    }

    /// The master key half.
    #[must_use]
    pub fn master_key(&self) -> &[u8] {
        let (key_len, _) = self.suite.key_and_salt_len();
        self.key_and_salt.get(..key_len).unwrap_or(&[])
    }

    /// The master salt half.
    #[must_use]
    pub fn master_salt(&self) -> &[u8] {
        let (key_len, _) = self.suite.key_and_salt_len();
        self.key_and_salt.get(key_len..).unwrap_or(&[])
    }

    /// Read an `a=crypto` value: `<tag> <suite> inline:<base64>[|lifetime][|mki]`.
    ///
    /// `None` for anything sipx cannot act on — an unknown suite, a key of the wrong length, a
    /// key parameter that is not `inline:`. Returning a half-understood offer would mean
    /// answering with a suite that cannot be performed.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split_whitespace();
        let tag: u32 = parts.next()?.parse().ok()?;
        let suite = Suite::parse(parts.next()?)?;

        // Several key parameters may be offered; sipx takes the first `inline:` one it can use.
        for parameter in parts {
            let Some(rest) = parameter.strip_prefix("inline:") else {
                continue;
            };
            // Lifetime and MKI follow the key, separated by `|`. sipx does not rekey, so the
            // lifetime is not acted on — but a key with one still has to be readable.
            let encoded = rest.split('|').next()?;
            let key_and_salt = base64_decode(encoded)?;
            let (key_len, salt_len) = suite.key_and_salt_len();
            if key_and_salt.len() != key_len + salt_len {
                return None;
            }
            return Some(Self {
                tag,
                suite,
                key_and_salt,
            });
        }
        None
    }

    /// This side's key, presented as the **accepted** attribute in an answer (RFC 4568 §5.1.2).
    ///
    /// The tag and the crypto-suite are the *offer's* — §5.1.2 requires the accepted attribute
    /// in the answer to "contain … the tag and crypto-suite from the accepted crypto attribute
    /// in the offer" — and the key is this side's own, because each direction is keyed
    /// separately (RFC 3711 §3.2).
    ///
    /// Answering with a tag of this side's choosing is not a cosmetic difference. A conformant
    /// offerer performs §5.1.3's check on the way back and MUST fail the negotiation when the
    /// tag it sent is not the tag it gets, so an endpoint that always answers `1` interoperates
    /// only with peers that happen to have offered `1`, and fails with no diagnosis at the end
    /// that is wrong.
    ///
    /// `None` when this side's key cannot be presented under the offered suite — a key of the
    /// wrong length for the suite named would be a well-formed answer nobody can decrypt.
    #[must_use]
    pub fn accepting(&self, offered: &Self) -> Option<Self> {
        let (key_len, salt_len) = offered.suite.key_and_salt_len();
        if self.key_and_salt.len() != key_len + salt_len {
            return None;
        }
        Some(Self {
            tag: offered.tag,
            suite: offered.suite,
            key_and_salt: self.key_and_salt.clone(),
        })
    }

    /// Check an answer against what was offered, and return the offered attribute it accepted
    /// (RFC 4568 §5.1.3).
    ///
    /// §5.1.3 is a MUST with three parts: the offerer verifies that one of the crypto suites it
    /// offered **and its accompanying tag** were echoed, and that the answer carries a key. "If
    /// any of the above fails, the negotiation MUST fail."
    ///
    /// `answered` is `None` when the answer carried no `a=crypto` this side can act on — which
    /// is how an answer naming a suite that was never offered arrives, since [`Crypto::parse`]
    /// refuses a suite sipx cannot perform. That is a failed negotiation and not a call in the
    /// clear: a media path that quietly drops to no encryption because the answer disagreed is
    /// worse than one that fails, because nothing tells anybody.
    ///
    /// What comes back is the *offered* attribute the answer accepted, so a caller keys with the
    /// half it actually sent rather than with whichever of its offers came first.
    ///
    /// # Errors
    ///
    /// [`crate::SdpError::Invalid`] naming the tag, and never the key material: an error string
    /// is a log line waiting to happen.
    pub fn verify_answer<'o>(
        offered: &'o [Self],
        answered: Option<&Self>,
    ) -> crate::Result<&'o Self> {
        let Some(answered) = answered else {
            return Err(crate::SdpError::Invalid {
                field: "crypto",
                value: "the answer carried no crypto attribute this side can perform".to_owned(),
            });
        };
        // Tag *and* suite together. §5.1.3 asks for both, and matching on the tag alone would
        // accept an answer that renamed the transform under a number this side did recognise.
        let accepted = offered
            .iter()
            .find(|ours| ours.tag == answered.tag && ours.suite == answered.suite)
            .ok_or_else(|| crate::SdpError::Invalid {
                field: "crypto",
                value: format!(
                    "the answer accepted tag {} ({}), which this side did not offer",
                    answered.tag,
                    answered.suite.as_str()
                ),
            })?;
        // "and that the answer contains a key". Half a keying is a stream that connects and
        // carries silence, which is the one outcome worse than a call that fails to connect.
        let (key_len, salt_len) = answered.suite.key_and_salt_len();
        if answered.key_and_salt.len() != key_len + salt_len {
            return Err(crate::SdpError::Invalid {
                field: "crypto",
                value: format!("the answer to tag {} carried no usable key", answered.tag),
            });
        }
        Ok(accepted)
    }

    /// Render as an `a=crypto` value.
    #[must_use]
    pub fn to_value(&self) -> String {
        format!(
            "{} {} inline:{}",
            self.tag,
            self.suite.as_str(),
            base64_encode(&self.key_and_salt)
        )
    }
}

/// Fill a buffer with cryptographically random bytes.
fn fill_random(out: &mut [u8]) {
    use rand::RngCore;
    rand::rng().fill_bytes(out);
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64, as RFC 4568 §6.1 requires for the inline parameter.
///
/// Hand-written rather than pulled in as a dependency: it is twenty lines, and the alternative
/// is another crate in the tree of a stack that carries audio.
fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let byte = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let group = (byte(0) << 16) | (byte(1) << 8) | byte(2);
        for i in 0..4 {
            if i <= chunk.len() {
                let index = ((group >> (18 - i * 6)) & 0x3F) as usize;
                // `index` is six bits and the alphabet is 64 long, so this cannot miss — but
                // written as a lookup rather than an index so that stays true if either changes.
                out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut held = 0u32;
    let mut out = Vec::with_capacity(text.len() * 3 / 4);

    for byte in text.bytes() {
        if byte == b'=' {
            break;
        }
        let value = ALPHABET.iter().position(|c| *c == byte)?;
        bits = (bits << 6) | u32::try_from(value).ok()?;
        held += 6;
        if held >= 8 {
            held -= 8;
            out.push(u8::try_from((bits >> held) & 0xFF).ok()?);
        }
    }
    Some(out)
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

    #[test]
    fn an_offer_round_trips() {
        let offer = Crypto::offer(1, Suite::AesCm128HmacSha1_80, true).expect("secure");
        let parsed = Crypto::parse(&offer.to_value()).expect("parses");
        assert_eq!(parsed, offer);
        assert_eq!(parsed.master_key().len(), 16);
        assert_eq!(parsed.master_salt().len(), 14);
    }

    /// The rule RFC 4568 §7.1 states and most implementations only document. A key in an SDP
    /// body is readable by anyone who can read the signalling, so an offer over cleartext SIP
    /// publishes it.
    #[test]
    fn no_key_is_offered_over_cleartext_signalling() {
        assert!(Crypto::offer(1, Suite::AesCm128HmacSha1_80, false).is_none());
        assert!(Crypto::offer(1, Suite::AesCm128HmacSha1_80, true).is_some());
    }

    /// Two offers must not share a key. A generator seeded once, or reused, gives every call on
    /// a host the same key — which authenticates and encrypts perfectly and protects nothing.
    #[test]
    fn every_offer_has_its_own_key() {
        let one = Crypto::offer(1, Suite::AesCm128HmacSha1_80, true).expect("secure");
        let two = Crypto::offer(1, Suite::AesCm128HmacSha1_80, true).expect("secure");
        assert_ne!(one.key_and_salt, two.key_and_salt);
        assert!(one.key_and_salt.iter().any(|b| *b != 0), "not all zeroes");
    }

    #[test]
    fn base64_matches_the_published_vectors() {
        // RFC 4648 §10.
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(
                base64_encode(plain.as_bytes()),
                encoded,
                "encoding {plain:?}"
            );
            assert_eq!(
                base64_decode(encoded).expect("decodes"),
                plain.as_bytes(),
                "decoding {encoded:?}"
            );
        }
    }

    #[test]
    fn a_lifetime_and_mki_do_not_stop_the_key_being_read() {
        let offer = Crypto::offer(3, Suite::AesCm128HmacSha1_80, true).expect("secure");
        let with_extras = format!("{}|2^20|1:4", offer.to_value());
        let parsed = Crypto::parse(&with_extras).expect("parses");
        assert_eq!(parsed.key_and_salt, offer.key_and_salt);
        assert_eq!(parsed.tag, 3);
    }

    /// Anything sipx cannot act on is refused rather than half-understood. Answering with a
    /// suite that cannot be performed is worse than not answering.
    #[test]
    fn an_offer_that_cannot_be_performed_is_refused() {
        assert!(
            Crypto::parse("1 AES_256_CM_HMAC_SHA1_80 inline:AAAA").is_none(),
            "unknown suite"
        );
        assert!(
            Crypto::parse("1 AES_CM_128_HMAC_SHA1_80 inline:AAAA").is_none(),
            "short key"
        );
        assert!(
            Crypto::parse("1 AES_CM_128_HMAC_SHA1_80").is_none(),
            "no key parameter"
        );
        assert!(
            Crypto::parse("x AES_CM_128_HMAC_SHA1_80 inline:AAAA").is_none(),
            "bad tag"
        );
        assert!(Crypto::parse("").is_none());
        // A key parameter that is not `inline:` — a key management protocol sipx does not speak.
        assert!(
            Crypto::parse("1 AES_CM_128_HMAC_SHA1_80 keymgmt:mikey AQAA").is_none(),
            "a keying method sipx cannot perform"
        );
    }

    /// **The published line, not one of ours.** `docs/specs/srtp.md` §10.4 restates RFC 4568
    /// §6.1's `a=crypto` example and what its `inline` parameter decodes to; this asserts
    /// `Crypto::parse` against those octets.
    ///
    /// Every other test in this module feeds the parser something [`Crypto::offer`] produced, so
    /// a parser that is self-consistently wrong reads as correct — which is exactly how
    /// `sipx-rtp` keyed HMAC with the wrong constant through six releases (§12.1).
    #[test]
    fn the_published_crypto_line_parses_to_the_published_key_and_salt() {
        let published = "1 AES_CM_128_HMAC_SHA1_80 \
                         inline:d0RmdmcmVCspeEc3QGZiNWpVLFJhQX1cfHAwJSoj|2^20|1:4";
        let parsed = Crypto::parse(published).expect("RFC 4568 §6.1's own example");

        assert_eq!(parsed.tag, 1);
        assert_eq!(parsed.suite, Suite::AesCm128HmacSha1_80);
        assert_eq!(
            parsed.master_key(),
            [
                0x77, 0x44, 0x66, 0x76, 0x67, 0x26, 0x54, 0x2B, 0x29, 0x78, 0x47, 0x37, 0x40, 0x66,
                0x62, 0x35
            ],
            "the 16 master key octets §10.4 publishes"
        );
        assert_eq!(
            parsed.master_salt(),
            [
                0x6A, 0x55, 0x2C, 0x52, 0x61, 0x41, 0x7D, 0x5C, 0x7C, 0x70, 0x30, 0x25, 0x2A, 0x23
            ],
            "the 14 master salt octets §10.4 publishes"
        );
    }

    /// The other two published `inline` parameters, from RFC 4568 §4 and §6.1. Both are 30
    /// octets and both are legal input — including the one whose lifetime is written in the
    /// decimal form rather than as a power of two.
    #[test]
    fn the_other_published_inline_parameters_are_read() {
        for value in [
            "1 AES_CM_128_HMAC_SHA1_80 inline:PS1uQCVeeCFCanVmcjkpPywjNWhcYD0mXXtxaVBR",
            "1 AES_CM_128_HMAC_SHA1_80 inline:YUJDZGVmZ2hpSktMbW9QUXJzVHVWd3l6MTIzNDU2|1066:4",
        ] {
            let parsed = Crypto::parse(value).unwrap_or_else(|| panic!("published: {value}"));
            assert_eq!(parsed.master_key().len(), 16, "{value}");
            assert_eq!(parsed.master_salt().len(), 14, "{value}");
        }
    }

    #[test]
    fn the_suite_token_is_case_sensitive() {
        assert!(Suite::parse("AES_CM_128_HMAC_SHA1_80").is_some());
        assert!(Suite::parse("aes_cm_128_hmac_sha1_80").is_none());
    }

    /// The key must not reach a log through a derived `Debug`, which for a key carried in
    /// signalling is the likeliest way it escapes.
    #[test]
    fn debug_output_does_not_leak_the_key() {
        let offer = Crypto::offer(1, Suite::AesCm128HmacSha1_80, true).expect("secure");
        let printed = format!("{offer:?}");
        assert!(printed.contains("tag: 1"), "{printed}");
        assert!(!printed.contains("key_and_salt"), "{printed}");
        let encoded = base64_encode(&offer.key_and_salt);
        assert!(!printed.contains(&encoded[..8]), "{printed}");
    }
}
