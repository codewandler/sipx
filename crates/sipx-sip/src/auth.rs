//! HTTP Digest authentication for SIP (RFC 7616, RFC 3261 §22).
//!
//! Digest is where a stack quietly fails to interoperate. The formula is simple; the ways to
//! get it wrong are not, and most of them produce a 401 loop rather than an error message:
//!
//! - `qop=auth` changes the response formula. A server that offers it and a client that
//!   ignores it compute different digests from the same password.
//! - The `-sess` algorithms hash `HA1` a second time with the nonces. Treating `MD5-sess` as
//!   `MD5` is a one-word mistake that authenticates against nothing.
//! - The nonce count must increase, and must be eight lowercase hex digits. A server that
//!   tracks it will reject a repeat as a replay.
//! - The `uri` in the credentials is the Request-URI of the request being authorized, not the
//!   URI of the user. They differ for REGISTER, which is the first request anyone tries.
//!
//! sipx supports MD5, MD5-sess, SHA-256 and SHA-256-sess. MD5 is not a defensible choice in
//! 2026, but it is what deployed registrars offer, and refusing it would mean refusing to
//! register. SHA-256 is preferred whenever the server offers it.

use std::fmt::Write as _;

use md5::Md5;
use sha2::{Digest, Sha256};

/// Which digest algorithm a challenge asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Algorithm {
    /// RFC 2617's original, and still what most registrars offer.
    #[default]
    Md5,
    /// MD5 with the session variant of `HA1`.
    Md5Sess,
    /// RFC 7616's preferred algorithm.
    Sha256,
    /// SHA-256 with the session variant of `HA1`.
    Sha256Sess,
    /// RFC 8760's addition: SHA-512/256, the truncated SHA-512 variant.
    ///
    /// Not SHA-512 truncated by hand — SHA-512/256 is a distinct function with its own initial
    /// values (FIPS 180-4 §5.3.6). Hashing with SHA-512 and taking the first half would produce
    /// a different digest and fail against every peer.
    Sha512_256,
    /// SHA-512/256 with the session variant of `HA1`.
    Sha512_256Sess,
}

impl Algorithm {
    /// Parse an `algorithm` parameter. An absent one means MD5 (RFC 7616 §3.3).
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "MD5" => Some(Self::Md5),
            "MD5-SESS" => Some(Self::Md5Sess),
            "SHA-256" => Some(Self::Sha256),
            "SHA-256-SESS" => Some(Self::Sha256Sess),
            "SHA-512-256" => Some(Self::Sha512_256),
            "SHA-512-256-SESS" => Some(Self::Sha512_256Sess),
            _ => None,
        }
    }

    /// How the algorithm spells itself in a header.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Md5 => "MD5",
            Self::Md5Sess => "MD5-sess",
            Self::Sha256 => "SHA-256",
            Self::Sha256Sess => "SHA-256-sess",
            Self::Sha512_256 => "SHA-512-256",
            Self::Sha512_256Sess => "SHA-512-256-sess",
        }
    }

    /// Whether `HA1` is hashed again with the nonces.
    #[must_use]
    pub fn is_session(self) -> bool {
        matches!(
            self,
            Self::Md5Sess | Self::Sha256Sess | Self::Sha512_256Sess
        )
    }

    /// How strong it is, for choosing among several offered challenges.
    ///
    /// The session variants rank above their plain forms because they bind `HA1` to this
    /// exchange's nonces, so a captured `HA1` cannot be replayed into a later one — a real
    /// property, not a longer digest.
    #[must_use]
    pub fn strength(self) -> u8 {
        match self {
            Self::Md5 => 1,
            Self::Md5Sess => 2,
            Self::Sha256 => 3,
            Self::Sha256Sess => 4,
            Self::Sha512_256 => 5,
            Self::Sha512_256Sess => 6,
        }
    }

    fn hash(self, input: &str) -> String {
        match self {
            Self::Md5 | Self::Md5Sess => hex(&Md5::digest(input.as_bytes())),
            Self::Sha256 | Self::Sha256Sess => hex(&Sha256::digest(input.as_bytes())),
            Self::Sha512_256 | Self::Sha512_256Sess => {
                hex(&sha2::Sha512_256::digest(input.as_bytes()))
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// A challenge from a `WWW-Authenticate` or `Proxy-Authenticate` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    /// The protection space.
    pub realm: String,
    /// The server's nonce.
    pub nonce: String,
    /// An opaque value to echo back verbatim.
    pub opaque: Option<String>,
    /// The algorithm; absent means MD5.
    pub algorithm: Algorithm,
    /// Whether the server offered `qop=auth`.
    pub qop_auth: bool,
    /// Whether the server says the nonce is merely stale, so the password is still good.
    pub stale: bool,
    /// Whether this came from a proxy, which decides the header used to answer it.
    pub from_proxy: bool,
}

impl Challenge {
    /// Parse one challenge header value.
    ///
    /// Returns `None` for a scheme that is not Digest — Basic exists in the grammar and must
    /// never be used for SIP, since it sends the password.
    #[must_use]
    pub fn parse(value: &[u8], from_proxy: bool) -> Option<Self> {
        let text = std::str::from_utf8(value).ok()?;
        let rest = text.trim().strip_prefix_ignore_ascii_case("Digest")?;

        let mut realm = None;
        let mut nonce = None;
        let mut opaque = None;
        let mut algorithm = Algorithm::Md5;
        let mut qop_auth = false;
        let mut stale = false;

        for (name, value) in params(rest) {
            match name.to_ascii_lowercase().as_str() {
                "realm" => realm = Some(value),
                "nonce" => nonce = Some(value),
                "opaque" => opaque = Some(value),
                "algorithm" => algorithm = Algorithm::parse(&value)?,
                // The value is a comma-separated list *inside* a quoted string, so the split
                // here is on its contents rather than on the header.
                "qop" => {
                    qop_auth = value
                        .split(',')
                        .any(|option| option.trim().eq_ignore_ascii_case("auth"));
                    if !qop_auth {
                        return None;
                    }
                }
                "stale" => stale = value.trim().eq_ignore_ascii_case("true"),
                _ => {}
            }
        }

        Some(Self {
            realm: realm?,
            nonce: nonce?,
            opaque,
            algorithm,
            qop_auth,
            stale,
            from_proxy,
        })
    }

    /// The header this challenge must be answered in.
    #[must_use]
    pub fn response_header(&self) -> crate::HeaderName {
        if self.from_proxy {
            crate::HeaderName::ProxyAuthorization
        } else {
            crate::HeaderName::Authorization
        }
    }
}

trait StripPrefixIgnoreCase {
    fn strip_prefix_ignore_ascii_case(&self, prefix: &str) -> Option<&str>;
}

impl StripPrefixIgnoreCase for str {
    fn strip_prefix_ignore_ascii_case(&self, prefix: &str) -> Option<&str> {
        let head = self.get(..prefix.len())?;
        head.eq_ignore_ascii_case(prefix)
            .then(|| self.get(prefix.len()..))
            .flatten()
    }
}

/// Split `name=value` pairs, honouring quoted strings.
///
/// Not a general parser: commas inside a quoted `qop="auth,auth-int"` must not split the list,
/// which is exactly the case a naive `split(',')` gets wrong.
fn params(input: &str) -> Vec<(String, String)> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        while matches!(bytes.get(i), Some(b' ' | b'\t' | b',')) {
            i += 1;
        }
        let name_start = i;
        while bytes.get(i).is_some_and(|&b| b != b'=' && b != b',') {
            i += 1;
        }
        let name = input.get(name_start..i).unwrap_or("").trim().to_owned();
        if bytes.get(i) != Some(&b'=') {
            if !name.is_empty() {
                out.push((name, String::new()));
            }
            continue;
        }
        i += 1;
        while matches!(bytes.get(i), Some(b' ' | b'\t')) {
            i += 1;
        }

        let value = if bytes.get(i) == Some(&b'"') {
            i += 1;
            let start = i;
            let mut unescaped = String::new();
            while let Some(&byte) = bytes.get(i) {
                match byte {
                    b'\\' => {
                        if let Some(&next) = bytes.get(i + 1) {
                            unescaped.push(char::from(next));
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    b'"' => break,
                    _ => {
                        unescaped.push(char::from(byte));
                        i += 1;
                    }
                }
            }
            let _ = start;
            i += 1;
            unescaped
        } else {
            let start = i;
            while bytes.get(i).is_some_and(|&b| b != b',') {
                i += 1;
            }
            input.get(start..i).unwrap_or("").trim().to_owned()
        };
        out.push((name, value));
    }
    out
}

/// What a user knows.
#[derive(Clone)]
pub struct Credentials {
    /// The username.
    pub username: String,
    /// The password.
    pub password: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish()
    }
}

impl Credentials {
    /// Credentials from a username and password.
    #[must_use]
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }
}

/// Answer a challenge.
///
/// `uri` is the Request-URI of the request being authorized — not the user's URI. They differ
/// for REGISTER, which is the first request anyone sends, so getting it wrong fails
/// immediately and confusingly.
#[must_use]
pub fn respond(
    challenge: &Challenge,
    credentials: &Credentials,
    method: &str,
    uri: &str,
    nonce_count: u32,
    cnonce: &str,
) -> String {
    let algorithm = challenge.algorithm;

    let mut ha1 = algorithm.hash(&format!(
        "{}:{}:{}",
        credentials.username, challenge.realm, credentials.password
    ));
    if algorithm.is_session() {
        // RFC 7616 §3.4.2: the session variants bind HA1 to this exchange's nonces, so a
        // captured HA1 cannot be replayed into a later one.
        ha1 = algorithm.hash(&format!("{ha1}:{}:{cnonce}", challenge.nonce));
    }

    let ha2 = algorithm.hash(&format!("{method}:{uri}"));

    let nc = format!("{nonce_count:08x}");
    let response = if challenge.qop_auth {
        algorithm.hash(&format!(
            "{ha1}:{}:{nc}:{cnonce}:auth:{ha2}",
            challenge.nonce
        ))
    } else {
        // The RFC 2069 formula. Still deployed, and the reason `qop` cannot simply be assumed.
        algorithm.hash(&format!("{ha1}:{}:{ha2}", challenge.nonce))
    };

    let mut header = format!(
        r#"Digest username="{}", realm="{}", nonce="{}", uri="{}", response="{response}""#,
        escape(&credentials.username),
        escape(&challenge.realm),
        escape(&challenge.nonce),
        escape(uri),
    );
    if challenge.qop_auth {
        let _ = write!(
            header,
            r#", qop=auth, nc={nc}, cnonce="{}""#,
            escape(cnonce)
        );
    }
    // An absent `algorithm` means MD5, but echoing it is harmless and some servers expect it.
    let _ = write!(header, ", algorithm={}", algorithm.as_str());
    if let Some(opaque) = &challenge.opaque {
        let _ = write!(header, r#", opaque="{}""#, escape(opaque));
    }
    header
}

/// Escape a value for a quoted string. A password or realm containing a quote would otherwise
/// end the string early and change what the rest of the header means.
fn escape(value: &str) -> String {
    value.replace('\\', r"\\").replace('"', "\\\"")
}

/// Pick the strongest challenge offered, which is a deliberate departure from RFC 8760 §2.4.
///
/// §2.4 says the UAC "SHOULD use the topmost header field that it supports **unless a local
/// policy dictates otherwise**". This is that local policy, and the reason is in §3 of the same
/// document: offering MD5 alongside a modern algorithm "opens the system to the potential for a
/// downgrade attack by an on-path attacker". A challenge is not integrity-protected, so an
/// attacker who can reorder the header fields can make the weakest algorithm topmost and a
/// client that honours the order will comply. Ranking by strength removes that lever entirely,
/// at the cost of ignoring a server's stated preference among algorithms it has already said it
/// accepts.
///
/// [`topmost_supported`] is the other policy, for a deployment where the server's ordering
/// carries information this client does not have.
///
/// Ties go to the earlier challenge, so the server's order still decides where strength does
/// not — and the result does not depend on how the header rows happened to be collected.
#[must_use]
pub fn strongest(challenges: Vec<Challenge>) -> Option<Challenge> {
    challenges.into_iter().reduce(|best, next| {
        if next.algorithm.strength() > best.algorithm.strength() {
            next
        } else {
            best
        }
    })
}

/// Pick the first challenge offered, which is RFC 8760 §2.4's own rule.
///
/// The server lists algorithms "in the order in which it would prefer to see them used" (§2.3),
/// and honouring that is the specified behaviour. Prefer [`strongest`] unless the ordering
/// genuinely carries information — see the downgrade note there for what this gives up.
///
/// Challenges the parser could not read never reach here: §2.4 also says "the client MUST
/// ignore any challenge it does not understand", and an unknown `algorithm` fails to parse into
/// a [`Challenge`] rather than being answered with the wrong hash.
#[must_use]
pub fn topmost_supported(challenges: Vec<Challenge>) -> Option<Challenge> {
    challenges.into_iter().next()
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

    /// RFC 2617 §3.5's worked example. The response below is the value the RFC itself
    /// publishes, so this test checks the implementation against the standard rather than
    /// against itself.
    #[test]
    fn rfc2617_worked_example_matches_the_published_digest() {
        let challenge = Challenge {
            realm: "testrealm@host.com".to_owned(),
            nonce: "dcd98b7102dd2f0e8b11d0f600bfb0c093".to_owned(),
            opaque: Some("5ccc069c403ebaf9f0171e9517f40e41".to_owned()),
            algorithm: Algorithm::Md5,
            qop_auth: true,
            stale: false,
            from_proxy: false,
        };
        let credentials = Credentials::new("Mufasa", "Circle Of Life");
        let header = respond(
            &challenge,
            &credentials,
            "GET",
            "/dir/index.html",
            1,
            "0a4f113b",
        );
        assert!(
            header.contains(r#"response="6629fae49393a05397450978507c4ef1""#),
            "must match the digest RFC 2617 publishes: {header}"
        );
        assert!(header.contains("nc=00000001"));
        assert!(header.contains("qop=auth"));
        assert!(header.contains(r#"opaque="5ccc069c403ebaf9f0171e9517f40e41""#));
    }

    /// RFC 7616 §3.9.1's worked example, verbatim.
    ///
    /// This replaced a test whose expected value had been "computed independently" — which is
    /// to say, computed by the same reasoning that wrote the code. A digest that agrees with
    /// itself proves nothing; this one agrees with the RFC.
    #[test]
    fn rfc7616_sha256_example_matches_the_published_digest() {
        let challenge = Challenge {
            realm: "http-auth@example.org".to_owned(),
            nonce: "7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v".to_owned(),
            opaque: Some("FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS".to_owned()),
            algorithm: Algorithm::Sha256,
            qop_auth: true,
            stale: false,
            from_proxy: false,
        };
        // Errata 4495 (verified): the password is "Circle of Life" with a lowercase "of",
        // where RFC 2617 had "Of". The §3.9.1 digest only reproduces with the lowercase form,
        // which is itself a check that the vector is being used rather than approximated.
        let header = respond(
            &challenge,
            &Credentials::new("Mufasa", "Circle of Life"),
            "GET",
            "/dir/index.html",
            1,
            "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ",
        );
        assert!(
            header.contains("753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1"),
            "must match the digest RFC 7616 §3.9.1 publishes: {header}"
        );
    }

    /// RFC 7616 §3.9.2's SHA-512-256 example, **as corrected by errata 4897**.
    ///
    /// The values printed in the RFC do not reproduce, and the erratum is still in "Reported"
    /// rather than "Verified" state, so neither source is authoritative on its own. Two things
    /// make this usable as a vector anyway: the erratum's `response` was arrived at
    /// independently by its reporter, and the erratum's *userhash* — a separate digest over
    /// different input — is asserted below and also reproduces. A pair of independent values
    /// agreeing is a much stronger signal than either one alone.
    ///
    /// The username carries a U+00E4 and a U+00F8 on purpose. `A1` is built from the raw
    /// UTF-8 octets, and an implementation that mangled the encoding would still pass an
    /// ASCII-only vector.
    #[test]
    fn rfc7616_sha512_256_example_matches_the_corrected_digest() {
        let challenge = Challenge {
            realm: "api@example.org".to_owned(),
            nonce: "5TsQWLVdgBdmrQ0XsxbDODV+57QdFR34I9HAbC/RVvkK".to_owned(),
            opaque: Some("HRPCssKJSGjCrkzDg8OhwpzCiGPChXYjwrI2QmXDnsOS".to_owned()),
            algorithm: Algorithm::Sha512_256,
            qop_auth: true,
            stale: false,
            from_proxy: false,
        };
        let header = respond(
            &challenge,
            &Credentials::new("J\u{e4}s\u{f8}n Doe", "Secret, or not?"),
            "GET",
            "/doe.json",
            1,
            "NTg6RKcb9boFIAS3KrFK9BGeh+iDa/sm6jUMp2wds69v",
        );
        assert!(
            header.contains("3798d4131c277846293534c3edc11bd8a5e4cdcbff78b05db9d95eeb1cec68a5"),
            "must match the digest errata 4897 publishes: {header}"
        );
    }

    #[test]
    fn sha512_256_is_the_fips_function_not_a_truncated_sha512() {
        // SHA-512/256 has its own initial hash values (FIPS 180-4 §5.3.6). Hashing with
        // SHA-512 and keeping the first 32 bytes gives a different answer, and a peer would
        // reject every response. The userhash from errata 4897 is the check: a second digest
        // over different input, from the same published example.
        let hashed = Algorithm::Sha512_256.hash("J\u{e4}s\u{f8}n Doe:api@example.org");
        assert_eq!(
            hashed,
            "793263caabb707a56211940d90411ea4a575adeccb7e360aeb624ed06ece9b0b"
        );
        let truncated_sha512 = {
            use sha2::{Digest as _, Sha512};
            hex(&Sha512::digest("J\u{e4}s\u{f8}n Doe:api@example.org".as_bytes())[..32])
        };
        assert_ne!(
            hashed, truncated_sha512,
            "SHA-512/256 must not be SHA-512 cut in half"
        );
    }

    /// The story's failing-first test.
    #[test]
    fn the_strongest_offered_algorithm_is_chosen() {
        let offer = |algorithm| Challenge {
            realm: "example.com".to_owned(),
            nonce: "n".to_owned(),
            opaque: None,
            algorithm,
            qop_auth: true,
            stale: false,
            from_proxy: false,
        };
        // A server that lists MD5 first — which RFC 8760 §2.3 lets it do, and which an on-path
        // attacker can also arrange by reordering, since the challenge is not integrity
        // protected.
        let chosen = strongest(vec![
            offer(Algorithm::Md5),
            offer(Algorithm::Sha256),
            offer(Algorithm::Sha512_256),
        ])
        .expect("one is chosen");
        assert_eq!(chosen.algorithm, Algorithm::Sha512_256);

        // And the other policy, which is §2.4's literal rule, answers the other way.
        let topmost = topmost_supported(vec![offer(Algorithm::Md5), offer(Algorithm::Sha512_256)])
            .expect("one is chosen");
        assert_eq!(topmost.algorithm, Algorithm::Md5);
    }

    #[test]
    fn an_equal_ranking_tie_goes_to_the_server_order() {
        // Where strength does not decide, the server's stated preference still does — and the
        // answer must not depend on which end of the list the iterator happened to reach last.
        let offer = |realm: &str| Challenge {
            realm: realm.to_owned(),
            nonce: "n".to_owned(),
            opaque: None,
            algorithm: Algorithm::Sha256,
            qop_auth: true,
            stale: false,
            from_proxy: false,
        };
        let chosen = strongest(vec![offer("first"), offer("second")]).expect("one is chosen");
        assert_eq!(chosen.realm, "first");
    }

    #[test]
    fn the_modern_algorithms_round_trip_their_names() {
        for algorithm in [
            Algorithm::Sha512_256,
            Algorithm::Sha512_256Sess,
            Algorithm::Sha256,
            Algorithm::Md5,
        ] {
            assert_eq!(
                Algorithm::parse(algorithm.as_str()),
                Some(algorithm),
                "{} did not survive a round trip",
                algorithm.as_str()
            );
        }
        // Case-insensitively, because servers spell it every way there is.
        assert_eq!(Algorithm::parse("sha-512-256"), Some(Algorithm::Sha512_256));
        assert_eq!(
            Algorithm::parse("SHA-512-256-SESS"),
            Some(Algorithm::Sha512_256Sess)
        );
        // §2.4: "the client MUST ignore any challenge it does not understand".
        assert_eq!(Algorithm::parse("SHA-3-512"), None);
    }

    /// `qop` changes the formula. A client that ignores it computes a different digest from
    /// the same password, and the server answers 401 again — forever.
    #[test]
    fn qop_changes_the_response() {
        let credentials = Credentials::new("alice", "secret");
        let base = Challenge {
            realm: "example.com".to_owned(),
            nonce: "abc123".to_owned(),
            opaque: None,
            algorithm: Algorithm::Md5,
            qop_auth: true,
            stale: false,
            from_proxy: false,
        };
        let with_qop = respond(&base, &credentials, "REGISTER", "sip:example.com", 1, "c");
        let without = respond(
            &Challenge {
                qop_auth: false,
                ..base
            },
            &credentials,
            "REGISTER",
            "sip:example.com",
            1,
            "c",
        );
        assert_ne!(
            digest_of(&with_qop),
            digest_of(&without),
            "the two formulas must not coincide"
        );
        assert!(!without.contains("qop"), "no qop offered, none sent");
        assert!(!without.contains("nc="), "and no nonce count either");
    }

    /// The session variants hash `HA1` again with the nonces. Treating `MD5-sess` as `MD5` is
    /// a one-word mistake that authenticates against nothing.
    #[test]
    fn the_session_variant_differs_from_the_plain_one() {
        let credentials = Credentials::new("alice", "secret");
        let plain = Challenge {
            realm: "example.com".to_owned(),
            nonce: "abc123".to_owned(),
            opaque: None,
            algorithm: Algorithm::Md5,
            qop_auth: true,
            stale: false,
            from_proxy: false,
        };
        let session = Challenge {
            algorithm: Algorithm::Md5Sess,
            ..plain.clone()
        };
        assert_ne!(
            digest_of(&respond(&plain, &credentials, "REGISTER", "sip:x", 1, "cn")),
            digest_of(&respond(
                &session,
                &credentials,
                "REGISTER",
                "sip:x",
                1,
                "cn"
            )),
        );
    }

    /// The nonce count is eight lowercase hex digits. A server that tracks it rejects anything
    /// else, and rejects a repeat as a replay.
    #[test]
    fn the_nonce_count_is_eight_hex_digits_and_advances() {
        let challenge = Challenge {
            realm: "r".to_owned(),
            nonce: "n".to_owned(),
            opaque: None,
            algorithm: Algorithm::Md5,
            qop_auth: true,
            stale: false,
            from_proxy: false,
        };
        let credentials = Credentials::new("u", "p");
        let first = respond(&challenge, &credentials, "REGISTER", "sip:x", 1, "cn");
        let second = respond(&challenge, &credentials, "REGISTER", "sip:x", 2, "cn");
        assert!(first.contains("nc=00000001"), "{first}");
        assert!(second.contains("nc=00000002"), "{second}");
        assert_ne!(
            digest_of(&first),
            digest_of(&second),
            "the count is part of the digest, so it must change the response"
        );

        let large = respond(
            &challenge,
            &credentials,
            "REGISTER",
            "sip:x",
            0x00ab_cdef,
            "cn",
        );
        assert!(large.contains("nc=00abcdef"), "lowercase hex: {large}");
    }

    #[test]
    fn a_challenge_parses_with_its_parameters_in_any_order() {
        let challenge = Challenge::parse(
            br#"Digest realm="example.com", qop="auth,auth-int", nonce="xyz", opaque="op", algorithm=SHA-256, stale=TRUE"#,
            false,
        )
        .expect("parses");
        assert_eq!(challenge.realm, "example.com");
        assert_eq!(challenge.nonce, "xyz");
        assert_eq!(challenge.opaque.as_deref(), Some("op"));
        assert_eq!(challenge.algorithm, Algorithm::Sha256);
        assert!(challenge.qop_auth, "auth is in the list");
        assert!(challenge.stale, "stale is case-insensitive");
    }

    #[test]
    fn an_auth_int_only_challenge_is_unsupported() {
        assert!(
            Challenge::parse(
                br#"Digest realm="example.com", nonce="xyz", qop="auth-int""#,
                false,
            )
            .is_none(),
            "sipx does not implement request-body integrity and must not answer with the legacy formula"
        );
    }

    /// The comma inside `qop="auth,auth-int"` is not a parameter separator. A parser that
    /// splits on commas first loses every parameter after it.
    #[test]
    fn a_comma_inside_a_quoted_value_does_not_split_the_parameters() {
        let challenge = Challenge::parse(
            br#"Digest realm="a,b", nonce="n,m", qop="auth,auth-int", opaque="last""#,
            false,
        )
        .expect("parses");
        assert_eq!(challenge.realm, "a,b");
        assert_eq!(challenge.nonce, "n,m");
        assert_eq!(
            challenge.opaque.as_deref(),
            Some("last"),
            "the parameter after the quoted list must survive"
        );
    }

    #[test]
    fn an_absent_algorithm_means_md5() {
        let challenge = Challenge::parse(br#"Digest realm="r", nonce="n""#, false).expect("parses");
        assert_eq!(challenge.algorithm, Algorithm::Md5);
        assert!(!challenge.qop_auth, "no qop offered");
    }

    /// Basic sends the password. It exists in the grammar and must never be answered.
    #[test]
    fn a_non_digest_scheme_is_refused() {
        assert!(Challenge::parse(b"Basic realm=\"example.com\"", false).is_none());
    }

    #[test]
    fn an_unknown_algorithm_is_refused_rather_than_guessed() {
        assert!(
            Challenge::parse(br#"Digest realm="r", nonce="n", algorithm=MD9"#, false).is_none()
        );
    }

    #[test]
    fn a_proxy_challenge_is_answered_in_the_proxy_header() {
        let direct = Challenge::parse(br#"Digest realm="r", nonce="n""#, false).expect("parses");
        let proxy = Challenge::parse(br#"Digest realm="r", nonce="n""#, true).expect("parses");
        assert_eq!(direct.response_header(), crate::HeaderName::Authorization);
        assert_eq!(
            proxy.response_header(),
            crate::HeaderName::ProxyAuthorization
        );
    }

    /// Answering the weakest of several offers is a downgrade the client chose for itself.
    #[test]
    fn the_strongest_offered_challenge_is_chosen() {
        let weak = Challenge::parse(br#"Digest realm="r", nonce="n", algorithm=MD5"#, false)
            .expect("parses");
        let strong = Challenge::parse(br#"Digest realm="r", nonce="n", algorithm=SHA-256"#, false)
            .expect("parses");
        assert_eq!(
            strongest(vec![weak.clone(), strong.clone()])
                .expect("one of them")
                .algorithm,
            Algorithm::Sha256
        );
        assert_eq!(
            strongest(vec![strong, weak])
                .expect("one of them")
                .algorithm,
            Algorithm::Sha256,
            "order of offer must not matter"
        );
    }

    /// A quote inside a value would end the string early and change what follows.
    #[test]
    fn quotes_in_a_value_are_escaped() {
        let challenge = Challenge {
            realm: r#"ex"ample"#.to_owned(),
            nonce: "n".to_owned(),
            opaque: None,
            algorithm: Algorithm::Md5,
            qop_auth: false,
            stale: false,
            from_proxy: false,
        };
        let header = respond(
            &challenge,
            &Credentials::new(r#"al"ice"#, "p"),
            "REGISTER",
            "sip:x",
            1,
            "cn",
        );
        assert!(header.contains(r#"username="al\"ice""#), "{header}");
        assert!(header.contains(r#"realm="ex\"ample""#), "{header}");
    }

    #[test]
    fn a_credentials_debug_report_never_contains_the_password() {
        let rendered = format!("{:?}", Credentials::new("alice", "Circle Of Life"));
        assert!(rendered.contains("alice"), "{rendered}");
        assert!(rendered.contains("[REDACTED]"), "{rendered}");
        assert!(!rendered.contains("Circle Of Life"), "{rendered}");
    }

    fn digest_of(header: &str) -> String {
        header
            .split("response=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_default()
            .to_owned()
    }
}
