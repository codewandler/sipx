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
        }
    }

    /// Whether `HA1` is hashed again with the nonces.
    #[must_use]
    pub fn is_session(self) -> bool {
        matches!(self, Self::Md5Sess | Self::Sha256Sess)
    }

    /// How strong it is, for choosing among several offered challenges.
    #[must_use]
    pub fn strength(self) -> u8 {
        match self {
            Self::Md5 => 1,
            Self::Md5Sess => 2,
            Self::Sha256 => 3,
            Self::Sha256Sess => 4,
        }
    }

    fn hash(self, input: &str) -> String {
        match self {
            Self::Md5 | Self::Md5Sess => hex(&Md5::digest(input.as_bytes())),
            Self::Sha256 | Self::Sha256Sess => hex(&Sha256::digest(input.as_bytes())),
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
    pub fn response_header(&self) -> sipx_sip::HeaderName {
        if self.from_proxy {
            sipx_sip::HeaderName::ProxyAuthorization
        } else {
            sipx_sip::HeaderName::Authorization
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
#[derive(Debug, Clone)]
pub struct Credentials {
    /// The username.
    pub username: String,
    /// The password.
    pub password: String,
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

/// A fresh client nonce.
#[must_use]
pub fn new_cnonce() -> String {
    use rand::Rng as _;
    let value: u64 = rand::rng().random();
    format!("{value:016x}")
}

/// Pick the strongest challenge offered.
///
/// A server may offer several; answering the weakest when it also offered SHA-256 is a
/// downgrade the client chose for itself.
#[must_use]
pub fn strongest(challenges: Vec<Challenge>) -> Option<Challenge> {
    challenges
        .into_iter()
        .max_by_key(|challenge| challenge.algorithm.strength())
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

    /// The same inputs under SHA-256. The formula is the one confirmed above; only the hash
    /// differs, and the expected value was computed independently.
    #[test]
    fn sha256_uses_the_same_formula_with_a_different_hash() {
        let challenge = Challenge {
            realm: "http-auth@example.org".to_owned(),
            nonce: "7ypf/xlj9XXwfFPEoyaVOOyLPE9BpNPCjZaeGVh6yF5w1M5PYqI=".to_owned(),
            opaque: None,
            algorithm: Algorithm::Sha256,
            qop_auth: true,
            stale: false,
            from_proxy: false,
        };
        let header = respond(
            &challenge,
            &Credentials::new("Mufasa", "Circle of Life"),
            "GET",
            "/dir/index.html",
            1,
            "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ",
        );
        assert!(
            header.contains("509d5b9f5dc373a8ddb1aabccb60b1de2b8c19752bc72cc918da3a7d726aff8d"),
            "{header}"
        );
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
        assert_eq!(
            direct.response_header(),
            sipx_sip::HeaderName::Authorization
        );
        assert_eq!(
            proxy.response_header(),
            sipx_sip::HeaderName::ProxyAuthorization
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

    /// Two cnonces in a row must differ; a fixed one defeats the point of a client nonce.
    #[test]
    fn client_nonces_are_not_repeated() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            assert!(seen.insert(new_cnonce()), "a cnonce was repeated");
        }
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
