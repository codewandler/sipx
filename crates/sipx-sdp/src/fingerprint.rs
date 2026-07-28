//! Certificate fingerprints and the TLS role, in SDP (RFC 8122, RFC 4145).
//!
//! This is the half of DTLS-SRTP that travels in the signalling. The key never does — that is the
//! entire point, and the difference from SDES ([`crate::crypto`], RFC 4568), where the key *is* the
//! SDP. Here the SDP carries only a **hash of the certificate** that will appear on the media path,
//! so a proxy or session border controller that terminates the TLS learns nothing it can decrypt
//! with. What it can do is substitute a fingerprint of its own; RFC 8122 §7 is clear that the
//! mechanism's guarantee is therefore only as good as the integrity of the signalling.
//!
//! The check is not optional. RFC 8122 §6.2: an endpoint whose peer's certificate "does not match
//! the original fingerprint" MUST "terminate the media connection with a `bad_certificate` error". A
//! stack that sends a fingerprint and does not verify one has implemented the decoration and not
//! the mechanism.

use std::fmt;

/// The hash a fingerprint was taken with (RFC 8122 §5).
///
/// MD2 and MD5 are deliberately absent. §5: implementations "MUST NOT use the MD2 and MD5 hash
/// functions to calculate fingerprints or to verify received fingerprints that have been calculated
/// using them". A parser that accepted them would be offering a caller a value it is forbidden to
/// act on, so they are rejected at the door instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashFunc {
    /// SHA-1. Accepted for interoperability with peers that still send it; not what sipx offers.
    Sha1,
    /// SHA-224.
    Sha224,
    /// SHA-256 — §5's preferred function, and what sipx sends.
    Sha256,
    /// SHA-384.
    Sha384,
    /// SHA-512.
    Sha512,
}

impl HashFunc {
    /// The token as it appears in SDP.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sha1 => "sha-1",
            Self::Sha224 => "sha-224",
            Self::Sha256 => "sha-256",
            Self::Sha384 => "sha-384",
            Self::Sha512 => "sha-512",
        }
    }

    /// The function a token names, if it is one that may be used.
    ///
    /// Case-insensitive: §5's grammar makes `hash-func` a token, and RFC 8866 §5 does not fix the
    /// case of attribute values. `md5` and `md2` parse to `None` — the grammar allows them and §5
    /// forbids acting on them, and returning `None` is how that prohibition is expressed here.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        [
            Self::Sha1,
            Self::Sha224,
            Self::Sha256,
            Self::Sha384,
            Self::Sha512,
        ]
        .into_iter()
        .find(|candidate| token.eq_ignore_ascii_case(candidate.as_str()))
    }

    /// How many octets the digest has.
    #[must_use]
    pub fn digest_len(self) -> usize {
        match self {
            Self::Sha1 => 20,
            Self::Sha224 => 28,
            Self::Sha256 => 32,
            Self::Sha384 => 48,
            Self::Sha512 => 64,
        }
    }

    /// Hash a certificate with this function.
    #[must_use]
    pub fn hash(self, certificate: &[u8]) -> Vec<u8> {
        use sha1::Sha1;
        use sha2::{Digest as _, Sha224, Sha256, Sha384, Sha512};
        match self {
            Self::Sha1 => Sha1::digest(certificate).to_vec(),
            Self::Sha224 => Sha224::digest(certificate).to_vec(),
            Self::Sha256 => Sha256::digest(certificate).to_vec(),
            Self::Sha384 => Sha384::digest(certificate).to_vec(),
            Self::Sha512 => Sha512::digest(certificate).to_vec(),
        }
    }
}

/// One `a=fingerprint` value (RFC 8122 §5).
#[derive(Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// Which hash it was taken with.
    pub func: HashFunc,
    /// The digest itself, as octets rather than as text — a fingerprint is compared to a hash of a
    /// certificate, and keeping it as the printed form would mean re-parsing to do that.
    pub digest: Vec<u8>,
}

impl fmt::Debug for Fingerprint {
    /// The digest is not a secret, but printing 64 hex pairs in a log line is noise. The function
    /// and the length are what a reader is looking for.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fingerprint")
            .field("func", &self.func)
            .field("digest_len", &self.digest.len())
            .finish()
    }
}

impl Fingerprint {
    /// The fingerprint of a DER-encoded certificate, taken with `func`.
    #[must_use]
    pub fn of(certificate: &[u8], func: HashFunc) -> Self {
        Self {
            func,
            digest: func.hash(certificate),
        }
    }

    /// Read an `a=fingerprint` value: `<hash-func> SP <2UHEX *(":" 2UHEX)>`.
    ///
    /// Returns `None` for a hash sipx may not act on, a malformed digest, or a digest whose length
    /// does not match the function named. The last check matters: a truncated digest that compared
    /// equal against the first bytes of a certificate hash would be a fingerprint check that
    /// verifies almost nothing.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.trim().split_ascii_whitespace();
        let func = HashFunc::parse(parts.next()?)?;
        let printed = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        let mut digest = Vec::with_capacity(func.digest_len());
        for pair in printed.split(':') {
            if pair.len() != 2 {
                return None;
            }
            digest.push(u8::from_str_radix(pair, 16).ok()?);
        }
        (digest.len() == func.digest_len()).then_some(Self { func, digest })
    }

    /// Render as an `a=fingerprint` value.
    ///
    /// Uppercase hex: §5's `UHEX` rule is `DIGIT / %x41-46`, which is uppercase only. Lowercase is
    /// what most implementations accept anyway, and is still not what the grammar says.
    #[must_use]
    pub fn to_value(&self) -> String {
        use std::fmt::Write as _;
        let printed =
            self.digest
                .iter()
                .enumerate()
                .fold(String::new(), |mut out, (index, byte)| {
                    if index > 0 {
                        out.push(':');
                    }
                    let _ = write!(out, "{byte:02X}");
                    out
                });
        format!("{} {printed}", self.func.as_str())
    }

    /// Whether a certificate is the one this fingerprint names (RFC 8122 §6.2).
    ///
    /// Compared in constant time. The value is public, so this is not about protecting the digest —
    /// it is about not giving an attacker who can offer certificates a byte-at-a-time oracle for
    /// how far a forged one matched.
    #[must_use]
    pub fn matches(&self, certificate: &[u8]) -> bool {
        use subtle::ConstantTimeEq as _;
        let computed = self.func.hash(certificate);
        computed.ct_eq(&self.digest).into()
    }
}

/// Who opens the DTLS connection (RFC 4145 §4, used by RFC 5763 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setup {
    /// This endpoint will start the handshake — the DTLS **client**.
    Active,
    /// This endpoint will wait for it — the DTLS **server**.
    Passive,
    /// Either; the answerer chooses. RFC 5763 §5 requires an *offerer* to use this.
    ActPass,
    /// No connection is to be formed, for an offer that is only describing a stream.
    HoldConn,
}

impl Setup {
    /// The token as it appears in SDP.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Passive => "passive",
            Self::ActPass => "actpass",
            Self::HoldConn => "holdconn",
        }
    }

    /// The role a token names.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        [Self::Active, Self::Passive, Self::ActPass, Self::HoldConn]
            .into_iter()
            .find(|candidate| token.trim().eq_ignore_ascii_case(candidate.as_str()))
    }

    /// The role to answer an offered one with (RFC 4145 §4.1).
    ///
    /// `actpass` is answered `active`, which is RFC 5763 §5's recommendation and not merely a
    /// preference: the answerer starting the handshake means the *offerer* does not have to send
    /// packets to an address it has only just learned, which is what gets a DTLS `ClientHello`
    /// through a NAT the answerer sits behind.
    ///
    /// `holdconn` is answered `holdconn`: §4.1 gives no other legal answer, and answering with a
    /// role would be agreeing to a connection the offerer said not to form.
    #[must_use]
    pub fn answer(offered: Self) -> Self {
        match offered {
            Self::ActPass | Self::Passive => Self::Active,
            Self::Active => Self::Passive,
            Self::HoldConn => Self::HoldConn,
        }
    }

    /// Whether holding this role makes this endpoint the DTLS client (RFC 8122 §6.2).
    #[must_use]
    pub fn is_client(self) -> bool {
        matches!(self, Self::Active)
    }
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

    /// A fingerprint round-trips through the printed form byte for byte.
    #[test]
    fn a_fingerprint_round_trips_through_its_sdp_form() {
        let certificate = b"a certificate, for the purposes of hashing something";
        let printed = Fingerprint::of(certificate, HashFunc::Sha256).to_value();
        let parsed = Fingerprint::parse(&printed).expect("parses");
        assert_eq!(parsed.func, HashFunc::Sha256);
        assert!(parsed.matches(certificate));
        assert_eq!(parsed.to_value(), printed);
    }

    /// RFC 8122 §5's `UHEX` rule is `DIGIT / %x41-46` — uppercase.
    #[test]
    fn the_printed_form_is_uppercase_hex_separated_by_colons() {
        let fingerprint = Fingerprint {
            func: HashFunc::Sha1,
            digest: vec![
                0xab, 0xcd, 0x01, 0x9f, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        };
        let value = fingerprint.to_value();
        assert!(value.starts_with("sha-1 AB:CD:01:9F:"), "{value}");
        assert_eq!(
            value.matches(':').count(),
            19,
            "twenty octets means nineteen separators"
        );
    }

    /// Lowercase is what most of the world sends, and rejecting it would be interoperability
    /// theatre — the grammar's case rule binds a *generator*.
    #[test]
    fn a_lowercase_fingerprint_from_a_peer_is_still_read() {
        let certificate = b"cert";
        let upper = Fingerprint::of(certificate, HashFunc::Sha256).to_value();
        let lower = upper.to_ascii_lowercase();
        let parsed = Fingerprint::parse(&lower).expect("a peer's lowercase value parses");
        assert!(parsed.matches(certificate));
    }

    /// §5: implementations "MUST NOT use the MD2 and MD5 hash functions to calculate fingerprints
    /// or to verify received fingerprints". Refused at the parser, so no caller can act on one.
    #[test]
    fn md5_and_md2_fingerprints_are_refused_rather_than_carried() {
        assert!(HashFunc::parse("md5").is_none());
        assert!(HashFunc::parse("md2").is_none());
        assert!(
            Fingerprint::parse("md5 AB:CD:EF:01:23:45:67:89:AB:CD:EF:01:23:45:67:89").is_none(),
            "a forbidden hash must not produce a fingerprint a caller could check against"
        );
    }

    /// A digest of the wrong length for the function named is malformed, not merely short. A
    /// truncated one that compared equal against a prefix would verify almost nothing.
    #[test]
    fn a_digest_of_the_wrong_length_for_its_hash_is_refused() {
        assert!(
            Fingerprint::parse("sha-256 AB:CD").is_none(),
            "two octets is not a SHA-256 digest"
        );
        let sha1_digest = Fingerprint::of(b"cert", HashFunc::Sha1).to_value();
        let mislabelled = sha1_digest.replace("sha-1", "sha-256");
        assert!(
            Fingerprint::parse(&mislabelled).is_none(),
            "a 20-octet digest labelled sha-256 must not be accepted"
        );
    }

    #[test]
    fn a_malformed_fingerprint_is_refused() {
        assert!(Fingerprint::parse("").is_none());
        assert!(Fingerprint::parse("sha-256").is_none(), "no digest");
        assert!(Fingerprint::parse("sha-256 ZZ:ZZ").is_none(), "not hex");
        assert!(
            Fingerprint::parse("sha-256 ABC:DE").is_none(),
            "groups are two hex digits"
        );
        let good = Fingerprint::of(b"cert", HashFunc::Sha256).to_value();
        assert!(
            Fingerprint::parse(&format!("{good} extra")).is_none(),
            "a trailing token is not part of the grammar"
        );
    }

    /// The check RFC 8122 §6.2 makes mandatory: a certificate that is not the one named must not
    /// match. This is the assertion the whole mechanism rests on.
    #[test]
    fn a_different_certificate_does_not_match() {
        let fingerprint = Fingerprint::of(b"the real certificate", HashFunc::Sha256);
        assert!(fingerprint.matches(b"the real certificate"));
        assert!(!fingerprint.matches(b"a substituted certificate"));
        // And a one-bit difference is still a difference.
        assert!(!fingerprint.matches(b"the real certificatf"));
    }

    /// Every hash sipx accepts produces a digest of the length it declares — the length check in
    /// `parse` is only as good as this table.
    #[test]
    fn every_hash_produces_the_digest_length_it_declares() {
        for func in [
            HashFunc::Sha1,
            HashFunc::Sha224,
            HashFunc::Sha256,
            HashFunc::Sha384,
            HashFunc::Sha512,
        ] {
            assert_eq!(
                func.hash(b"cert").len(),
                func.digest_len(),
                "{}",
                func.as_str()
            );
        }
    }

    /// RFC 4145 §4.1, and RFC 5763 §5's reason for preferring it.
    #[test]
    fn actpass_is_answered_active_so_the_answerer_starts_the_handshake() {
        assert_eq!(Setup::answer(Setup::ActPass), Setup::Active);
        assert!(
            Setup::answer(Setup::ActPass).is_client(),
            "the answerer becomes the DTLS client, so its `ClientHello` opens the NAT it is behind"
        );
        assert_eq!(Setup::answer(Setup::Passive), Setup::Active);
        assert_eq!(Setup::answer(Setup::Active), Setup::Passive);
    }

    /// §4.1 gives no other legal answer to `holdconn`, and answering with a role would agree to a
    /// connection the offerer said not to form.
    #[test]
    fn holdconn_is_answered_holdconn() {
        assert_eq!(Setup::answer(Setup::HoldConn), Setup::HoldConn);
        assert!(!Setup::answer(Setup::HoldConn).is_client());
    }

    #[test]
    fn setup_round_trips_and_rejects_what_is_not_a_role() {
        for role in [
            Setup::Active,
            Setup::Passive,
            Setup::ActPass,
            Setup::HoldConn,
        ] {
            assert_eq!(Setup::parse(role.as_str()), Some(role));
            assert_eq!(
                Setup::parse(&role.as_str().to_ascii_uppercase()),
                Some(role)
            );
        }
        assert!(Setup::parse("both").is_none());
        assert!(Setup::parse("").is_none());
    }

    #[test]
    fn only_active_is_the_client() {
        assert!(Setup::Active.is_client());
        assert!(!Setup::Passive.is_client());
        // `actpass` is an offer, not a role. Treating it as a role is how both ends end up as
        // clients and neither answers.
        assert!(!Setup::ActPass.is_client());
        assert!(!Setup::HoldConn.is_client());
    }
}
