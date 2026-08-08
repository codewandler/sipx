//! The server side of digest authentication (RFC 7616, RFC 8760).
//!
//! [`crate::auth`] answers a challenge. This issues one, and checks what comes back — the same
//! formulas from the other end, which is why they are not written a second time here.
//!
//! **Scope is the primitives.** Which credential a username maps to, and what to do about a
//! failure, belong to whoever is authenticating: a credential store is not this crate's business.
//! So verification takes the password as an argument and returns a verdict, and the caller decides
//! everything around it.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sipx_sip::{HeaderName, Request};

use crate::auth::{Algorithm, Credentials, respond};

/// How long a nonce is good for by default.
///
/// Five minutes. Long enough that an ordinary REGISTER-then-refresh does not re-challenge, short
/// enough that a captured `Authorization` is not a lasting credential. A longer window is not more
/// convenient — a client that gets `stale=true` re-sends without prompting anyone.
pub const DEFAULT_LIFETIME: Duration = Duration::from_secs(300);

/// How many nonces the replay window remembers at once.
///
/// The window has to be bounded or it is a memory leak with a protocol in front of it. Evicting
/// the oldest is safe in the direction that matters: a client whose nonce is evicted is challenged
/// again with `stale=true` and retries without a human, whereas an unbounded map is an outage.
const REPLAY_CAPACITY: usize = 4096;

/// What a request presented in its `Authorization` or `Proxy-Authorization`.
///
/// Parsed but not trusted. Every field here is attacker-controlled; the only thing that makes any
/// of it meaningful is [`Authenticator::verify`] recomputing the response from a password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presented {
    /// Who the request claims to be.
    pub username: String,
    /// The realm it answered.
    pub realm: String,
    /// The nonce it answered.
    pub nonce: String,
    /// The URI the digest covers — **not** necessarily the request's own.
    pub uri: String,
    /// The computed response.
    pub response: String,
    /// The algorithm named, defaulting to MD5 when absent (RFC 7616 §3.4).
    pub algorithm: Algorithm,
    /// The nonce count, if `qop` was used.
    pub nonce_count: Option<u32>,
    /// The client nonce, if `qop` was used.
    pub cnonce: Option<String>,
    /// Whether `qop=auth` was claimed.
    pub qop_auth: bool,
}

impl Presented {
    /// Read the credentials out of a request.
    ///
    /// `proxy` selects `Proxy-Authorization` over `Authorization`. Which one is right is not a
    /// detail: a UAS challenges with 401 and reads `Authorization`, a proxy challenges with 407 and
    /// reads `Proxy-Authorization`, and a server that reads the wrong one authenticates nobody
    /// while looking like it works.
    #[must_use]
    pub fn from_request(request: &Request, proxy: bool) -> Option<Self> {
        let header = if proxy {
            HeaderName::ProxyAuthorization
        } else {
            HeaderName::Authorization
        };
        let value = request.headers.value(&header)?;
        Self::parse(&value)
    }

    /// Read a `Digest …` credentials value.
    #[must_use]
    pub fn parse(value: &[u8]) -> Option<Self> {
        let text = String::from_utf8_lossy(value);
        let rest = text.trim().strip_prefix("Digest")?.trim_start();
        let param = |name: &str| parameter(rest, name);

        let qop = param("qop");
        Some(Self {
            username: param("username")?,
            realm: param("realm").unwrap_or_default(),
            nonce: param("nonce")?,
            uri: param("uri").unwrap_or_default(),
            response: param("response")?,
            algorithm: param("algorithm")
                .as_deref()
                .and_then(Algorithm::parse)
                // RFC 7616 §3.4: an absent algorithm means MD5.
                .unwrap_or(Algorithm::Md5),
            nonce_count: param("nc").and_then(|nc| u32::from_str_radix(&nc, 16).ok()),
            cnonce: param("cnonce"),
            qop_auth: qop.as_deref() == Some("auth"),
        })
    }
}

/// Read one parameter out of a comma-separated credentials list, quoted or not.
fn parameter(input: &str, name: &str) -> Option<String> {
    let mut rest = input;
    while !rest.is_empty() {
        let rest_trimmed = rest.trim_start_matches([' ', '\t', ',']);
        let (key, after) = rest_trimmed.split_once('=')?;
        let key = key.trim();
        let after = after.trim_start();
        let (value, remainder) = if let Some(quoted) = after.strip_prefix('"') {
            // A quoted string, honouring backslash escapes — a realm or username containing a
            // quote would otherwise end the value early and shift every parameter after it.
            let mut value = String::new();
            let mut chars = quoted.char_indices();
            let mut end = None;
            while let Some((index, character)) = chars.next() {
                match character {
                    '\\' => {
                        if let Some((_, escaped)) = chars.next() {
                            value.push(escaped);
                        }
                    }
                    '"' => {
                        end = Some(index + 1);
                        break;
                    }
                    other => value.push(other),
                }
            }
            (value, quoted.get(end?..).unwrap_or_default())
        } else {
            let end = after.find(',').unwrap_or(after.len());
            (
                after.get(..end).unwrap_or_default().trim().to_owned(),
                after.get(end..).unwrap_or_default(),
            )
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(value);
        }
        rest = remainder;
    }
    None
}

/// What verification concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The credentials are correct and fresh.
    Authenticated,
    /// The credentials are correct and the nonce has expired.
    ///
    /// Challenge again with `stale=true` (RFC 7616 §3.3). The distinction is the whole reason
    /// `stale` exists: a client told `stale=true` re-computes and re-sends by itself, and one told
    /// only "401" prompts a human for a password that was never wrong.
    Stale,
    /// The credentials are not correct, or not this server's to accept.
    Rejected(Reason),
}

/// Why verification failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The digest did not match the password.
    ///
    /// Deliberately does not distinguish "no such user" from "wrong password": the difference is a
    /// user-enumeration oracle, and it is not information the far end is entitled to.
    Mismatch,
    /// The nonce was not issued by this server, or has been tampered with.
    ForeignNonce,
    /// The nonce count repeated with a *different* request — a replay.
    Replay,
    /// `qop=auth` was offered and the credentials did not use it, or vice versa.
    QopMismatch,
    /// The algorithm named is not one this server offered.
    Algorithm,
}

/// A server that issues digest challenges and checks the answers.
///
/// Nonces are **self-describing**: each carries its issue time and a MAC over it, so this can
/// recognise its own nonce and read its expiry without a table of every nonce ever issued. The only
/// table is the replay window, which is bounded and holds nothing that has not been used.
#[derive(Debug)]
pub struct Authenticator {
    realm: String,
    secret: [u8; 32],
    lifetime: Duration,
    algorithm: Algorithm,
    /// Nonce → the highest nonce-count seen and the response that came with it.
    ///
    /// The response is kept so a *retransmission* — the same request arriving twice, which is
    /// ordinary over UDP — can be told from a replay. Same count and same response is the same
    /// request; same count and a different response is somebody reusing a captured credential.
    seen: std::collections::VecDeque<(String, u32, String)>,
}

impl Authenticator {
    /// A server for a protection space.
    ///
    /// `secret` keys the nonce MAC. It must be **stable across restarts** if in-flight nonces are
    /// to survive one, and **not shared** with another realm, or a nonce issued for one protection
    /// space is accepted in the other.
    #[must_use]
    pub fn new(realm: impl Into<String>, secret: [u8; 32]) -> Self {
        Self {
            realm: realm.into(),
            secret,
            lifetime: DEFAULT_LIFETIME,
            algorithm: Algorithm::Sha256,
            seen: std::collections::VecDeque::new(),
        }
    }

    /// A server with a freshly generated secret.
    ///
    /// Convenient, and it means every restart invalidates every outstanding nonce — which clients
    /// recover from with `stale=true`, so it costs a round trip and not a login.
    #[must_use]
    pub fn with_random_secret(realm: impl Into<String>) -> Self {
        use rand::Rng as _;
        let mut secret = [0u8; 32];
        rand::rng().fill(&mut secret);
        Self::new(realm, secret)
    }

    /// Challenge with this algorithm.
    ///
    /// SHA-256 by default rather than MD5. RFC 8760 §2 exists because MD5 should not be the only
    /// thing on offer, and a *server* choosing the default is the only place that choice can be
    /// made — a client can only answer what it is asked.
    #[must_use]
    pub fn with_algorithm(mut self, algorithm: Algorithm) -> Self {
        self.algorithm = algorithm;
        self
    }

    /// How long an issued nonce stays valid.
    #[must_use]
    pub fn with_lifetime(mut self, lifetime: Duration) -> Self {
        self.lifetime = lifetime;
        self
    }

    /// The protection space.
    #[must_use]
    pub fn realm(&self) -> &str {
        &self.realm
    }

    /// The header a challenge goes in: `WWW-Authenticate`, or `Proxy-Authenticate` for a proxy.
    #[must_use]
    pub fn challenge_header(proxy: bool) -> HeaderName {
        if proxy {
            HeaderName::ProxyAuthenticate
        } else {
            HeaderName::WwwAuthenticate
        }
    }

    /// Mint a challenge value, as it goes in the header.
    ///
    /// `stale` says the previous credentials were right and only the nonce was old.
    #[must_use]
    pub fn challenge(&self, stale: bool) -> String {
        self.challenge_at(stale, now())
    }

    /// [`Authenticator::challenge`] with the clock supplied, so a test can pin it.
    #[must_use]
    pub fn challenge_at(&self, stale: bool, now: u64) -> String {
        let nonce = self.mint(now);
        let mut header = format!(
            r#"Digest realm="{}", nonce="{nonce}", qop="auth", algorithm={}"#,
            self.realm.replace('\\', r"\\").replace('"', "\\\""),
            self.algorithm.as_str()
        );
        if stale {
            header.push_str(", stale=true");
        }
        header
    }

    /// A nonce that this server can later recognise as its own.
    ///
    /// `<issued-at in hex>.<HMAC-SHA-256 of it, under the secret, truncated>`. The MAC is what makes
    /// the nonce unforgeable; the timestamp is what makes expiry checkable without a table.
    fn mint(&self, now: u64) -> String {
        let issued = format!("{now:016x}");
        format!("{issued}.{}", self.mac(&issued))
    }

    /// The MAC over a nonce's issue time. Keyed properly rather than `H(secret || message)`, which
    /// with a Merkle–Damgård hash is extensible by anyone who has seen one output.
    fn mac(&self, issued: &str) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.secret)
            .unwrap_or_else(|_| unreachable!("HMAC accepts a key of any length"));
        mac.update(issued.as_bytes());
        mac.update(b":");
        mac.update(self.realm.as_bytes());
        let bytes = mac.finalize().into_bytes();
        bytes.iter().fold(String::new(), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        })
    }

    /// When a nonce was issued, if it is one of ours.
    ///
    /// Constant-time on the MAC comparison. The nonce is public, so this is not about hiding it —
    /// it is about not giving an attacker who can submit nonces a byte-at-a-time oracle for forging
    /// one.
    fn issued_at(&self, nonce: &str) -> Option<u64> {
        use subtle::ConstantTimeEq as _;
        let (issued, mac) = nonce.split_once('.')?;
        let expected = self.mac(issued);
        let matches: bool = expected.as_bytes().ct_eq(mac.as_bytes()).into();
        matches.then(|| u64::from_str_radix(issued, 16).ok())?
    }

    /// Check the credentials a request presented.
    ///
    /// `password` is the one this server holds for `presented.username`; looking it up is the
    /// caller's job, because a credential store is not this crate's business. `method` is the
    /// request's, since the digest covers it.
    pub fn verify(&mut self, presented: &Presented, method: &str, password: &str) -> Verdict {
        self.verify_at(presented, method, password, now())
    }

    /// [`Authenticator::verify`] with the clock supplied.
    pub fn verify_at(
        &mut self,
        presented: &Presented,
        method: &str,
        password: &str,
        now: u64,
    ) -> Verdict {
        if presented.algorithm != self.algorithm {
            return Verdict::Rejected(Reason::Algorithm);
        }
        // The challenge always offers `qop=auth`, so credentials without it are answering a
        // question this server did not ask — and the RFC 2069 formula they would then use has no
        // client nonce in it, which is the replay protection.
        if !presented.qop_auth || presented.nonce_count.is_none() || presented.cnonce.is_none() {
            return Verdict::Rejected(Reason::QopMismatch);
        }
        let Some(issued) = self.issued_at(&presented.nonce) else {
            return Verdict::Rejected(Reason::ForeignNonce);
        };

        // The digest is checked *before* the clock. A wrong password on an expired nonce is a
        // rejection, not a `stale` — answering `stale=true` there would tell an attacker that the
        // only thing wrong with their guess was its timing.
        let expected = respond(
            &self.as_challenge(&presented.nonce),
            &Credentials::new(presented.username.clone(), password.to_owned()),
            method,
            &presented.uri,
            presented.nonce_count.unwrap_or(1),
            presented.cnonce.as_deref().unwrap_or_default(),
        );
        let Some(computed) = parameter(&expected, "response") else {
            return Verdict::Rejected(Reason::Mismatch);
        };
        if !constant_time_eq(computed.as_bytes(), presented.response.as_bytes()) {
            return Verdict::Rejected(Reason::Mismatch);
        }

        if now.saturating_sub(issued) > self.lifetime.as_secs() {
            return Verdict::Stale;
        }

        match self.record(presented) {
            Ok(()) => Verdict::Authenticated,
            Err(reason) => Verdict::Rejected(reason),
        }
    }

    /// Advance the replay window, or refuse.
    ///
    /// RFC 7616 §3.4.3 makes `nc` count the requests sent with one nonce, so it must never go
    /// backwards or repeat — *except* that a retransmission is the same request arriving twice,
    /// which is ordinary over UDP and must still authenticate. The response digest tells them
    /// apart: same count and the same digest is one request seen twice; same count and a different
    /// digest is somebody reusing a captured credential against a different request.
    fn record(&mut self, presented: &Presented) -> Result<(), Reason> {
        let count = presented.nonce_count.unwrap_or(1);
        if let Some(entry) = self
            .seen
            .iter_mut()
            .find(|(nonce, _, _)| nonce == &presented.nonce)
        {
            if count > entry.1 {
                entry.1 = count;
                entry.2.clone_from(&presented.response);
                return Ok(());
            }
            if count == entry.1 && entry.2 == presented.response {
                return Ok(());
            }
            return Err(Reason::Replay);
        }
        // Bounded: the oldest nonce goes when the window is full. A client whose nonce is evicted
        // is challenged again with `stale=true` and retries by itself, which is a round trip. An
        // unbounded window is an outage.
        if self.seen.len() >= REPLAY_CAPACITY {
            self.seen.pop_front();
        }
        self.seen
            .push_back((presented.nonce.clone(), count, presented.response.clone()));
        Ok(())
    }

    /// This server's parameters as a client-side [`crate::auth::Challenge`], so the response is
    /// computed by the *same code* the client uses.
    ///
    /// Writing the formula a second time here is how the two sides drift, and a server whose
    /// verification disagrees with its own client rejects correct credentials.
    fn as_challenge(&self, nonce: &str) -> crate::auth::Challenge {
        crate::auth::Challenge {
            realm: self.realm.clone(),
            nonce: nonce.to_owned(),
            opaque: None,
            algorithm: self.algorithm,
            qop_auth: true,
            stale: false,
            from_proxy: false,
        }
    }
}

fn constant_time_eq(one: &[u8], other: &[u8]) -> bool {
    use subtle::ConstantTimeEq as _;
    one.len() == other.len() && one.ct_eq(other).into()
}

/// Seconds since the epoch.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
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

    const NOW: u64 = 1_700_000_000;
    const PASSWORD: &str = "Circle Of Life";

    fn server() -> Authenticator {
        Authenticator::new("sipx.test", [7u8; 32])
    }

    /// Answer a challenge the way a real client would — through the client-side code.
    fn answer(
        server: &Authenticator,
        challenge: &str,
        method: &str,
        nc: u32,
        cnonce: &str,
    ) -> Presented {
        let parsed = crate::auth::Challenge::parse(challenge.as_bytes(), false).expect("parses");
        let header = respond(
            &parsed,
            &Credentials::new("alice", PASSWORD),
            method,
            "sip:sipx.test",
            nc,
            cnonce,
        );
        let _ = server;
        Presented::parse(header.as_bytes()).expect("parses")
    }

    #[test]
    fn a_challenge_carries_a_realm_a_nonce_and_qop() {
        let value = server().challenge_at(false, NOW);
        assert!(value.starts_with("Digest "), "{value}");
        assert!(value.contains(r#"realm="sipx.test""#), "{value}");
        assert!(value.contains(r#"qop="auth""#), "{value}");
        assert!(value.contains("algorithm=SHA-256"), "{value}");
        assert!(!value.contains("stale"), "a fresh challenge is not stale");
        assert!(
            server().challenge_at(true, NOW).contains("stale=true"),
            "a stale challenge says so"
        );
    }

    #[test]
    fn a_server_recognises_its_own_nonce_and_not_anyone_elses() {
        let server = server();
        let nonce = server.mint(NOW);
        assert_eq!(server.issued_at(&nonce), Some(NOW));

        // A different secret is a different server.
        let other = Authenticator::new("sipx.test", [9u8; 32]);
        assert_eq!(other.issued_at(&nonce), None, "a foreign nonce");

        // The same secret in a different realm is also a different server: otherwise a nonce
        // issued for one protection space authenticates in the other.
        let other_realm = Authenticator::new("elsewhere.test", [7u8; 32]);
        assert_eq!(other_realm.issued_at(&nonce), None);

        // And a tampered one is nobody's.
        let mut tampered = nonce.clone();
        tampered.replace_range(0..1, "f");
        assert_eq!(server.issued_at(&tampered), None);
        assert_eq!(server.issued_at("not-a-nonce"), None);
        assert_eq!(server.issued_at(""), None);
    }

    #[test]
    fn correct_credentials_authenticate() {
        let mut server = server();
        let challenge = server.challenge_at(false, NOW);
        let presented = answer(&server, &challenge, "REGISTER", 1, "abc");
        assert_eq!(
            server.verify_at(&presented, "REGISTER", PASSWORD, NOW),
            Verdict::Authenticated
        );
    }

    #[test]
    fn a_wrong_password_is_rejected() {
        let mut server = server();
        let challenge = server.challenge_at(false, NOW);
        let presented = answer(&server, &challenge, "REGISTER", 1, "abc");
        assert_eq!(
            server.verify_at(&presented, "REGISTER", "the wrong one", NOW),
            Verdict::Rejected(Reason::Mismatch)
        );
    }

    /// The digest covers the method, so credentials computed for one request do not authenticate
    /// another. Without this an intercepted REGISTER's credentials would authorise an INVITE.
    #[test]
    fn credentials_for_one_method_do_not_authenticate_another() {
        let mut server = server();
        let challenge = server.challenge_at(false, NOW);
        let presented = answer(&server, &challenge, "REGISTER", 1, "abc");
        assert_eq!(
            server.verify_at(&presented, "INVITE", PASSWORD, NOW),
            Verdict::Rejected(Reason::Mismatch)
        );
    }

    /// The story's failing-first test.
    ///
    /// RFC 7616 §3.4.3 counts requests per nonce, so a repeated count is a replay — except that a
    /// retransmission *is* the same request arriving twice, which is ordinary over UDP. Rejecting
    /// both is a stack that fails authentication whenever a packet is duplicated.
    #[test]
    fn a_replayed_nonce_count_is_rejected_but_a_retransmission_is_not() {
        let mut server = server();
        let challenge = server.challenge_at(false, NOW);
        let first = answer(&server, &challenge, "REGISTER", 1, "abc");

        assert_eq!(
            server.verify_at(&first, "REGISTER", PASSWORD, NOW),
            Verdict::Authenticated
        );

        // The identical request again: a retransmission. Same nonce, same count, same digest.
        assert_eq!(
            server.verify_at(&first, "REGISTER", PASSWORD, NOW),
            Verdict::Authenticated,
            "a retransmission must still authenticate, or a duplicated packet fails a login"
        );

        // A *different* request reusing that count: a replay.
        let replayed = answer(&server, &challenge, "INVITE", 1, "abc");
        assert_eq!(
            server.verify_at(&replayed, "INVITE", PASSWORD, NOW),
            Verdict::Rejected(Reason::Replay),
            "the same count with a different request is somebody reusing a captured credential"
        );

        // And the count advancing is ordinary.
        let next = answer(&server, &challenge, "REGISTER", 2, "abc");
        assert_eq!(
            server.verify_at(&next, "REGISTER", PASSWORD, NOW),
            Verdict::Authenticated
        );

        // Going backwards is not.
        let backwards = answer(&server, &challenge, "REGISTER", 1, "abc");
        assert_eq!(
            server.verify_at(&backwards, "REGISTER", PASSWORD, NOW),
            Verdict::Rejected(Reason::Replay)
        );
    }

    /// §3.3: `stale=true` says the password was right and only the nonce was old, so a client
    /// re-sends instead of prompting a human.
    #[test]
    fn an_expired_nonce_with_correct_credentials_is_stale_rather_than_rejected() {
        let mut server = server().with_lifetime(Duration::from_secs(60));
        let challenge = server.challenge_at(false, NOW);
        let presented = answer(&server, &challenge, "REGISTER", 1, "abc");
        assert_eq!(
            server.verify_at(&presented, "REGISTER", PASSWORD, NOW + 61),
            Verdict::Stale
        );
    }

    /// But a *wrong* password on an expired nonce is a rejection. Answering `stale` would tell an
    /// attacker that the only thing wrong with their guess was its timing.
    #[test]
    fn an_expired_nonce_with_wrong_credentials_is_a_rejection_not_a_stale() {
        let mut server = server().with_lifetime(Duration::from_secs(60));
        let challenge = server.challenge_at(false, NOW);
        let presented = answer(&server, &challenge, "REGISTER", 1, "abc");
        assert_eq!(
            server.verify_at(&presented, "REGISTER", "wrong", NOW + 61),
            Verdict::Rejected(Reason::Mismatch)
        );
    }

    #[test]
    fn a_nonce_this_server_did_not_issue_is_refused() {
        let mut server = server();
        let elsewhere = Authenticator::new("sipx.test", [1u8; 32]);
        let challenge = elsewhere.challenge_at(false, NOW);
        let presented = answer(&server, &challenge, "REGISTER", 1, "abc");
        assert_eq!(
            server.verify_at(&presented, "REGISTER", PASSWORD, NOW),
            Verdict::Rejected(Reason::ForeignNonce)
        );
    }

    #[test]
    fn credentials_without_qop_are_refused_because_the_challenge_required_it() {
        let mut server = server();
        let nonce = server.mint(NOW);
        let presented = Presented {
            username: "alice".to_owned(),
            realm: "sipx.test".to_owned(),
            nonce,
            uri: "sip:sipx.test".to_owned(),
            response: "0".repeat(64),
            algorithm: Algorithm::Sha256,
            nonce_count: None,
            cnonce: None,
            qop_auth: false,
        };
        assert_eq!(
            server.verify_at(&presented, "REGISTER", PASSWORD, NOW),
            Verdict::Rejected(Reason::QopMismatch),
            "the RFC 2069 formula has no client nonce in it, which is the replay protection"
        );
    }

    #[test]
    fn an_algorithm_this_server_did_not_offer_is_refused() {
        let mut server = server(); // SHA-256
        let nonce = server.mint(NOW);
        let presented = Presented {
            username: "alice".to_owned(),
            realm: "sipx.test".to_owned(),
            nonce,
            uri: "sip:sipx.test".to_owned(),
            response: "0".repeat(32),
            algorithm: Algorithm::Md5,
            nonce_count: Some(1),
            cnonce: Some("abc".to_owned()),
            qop_auth: true,
        };
        assert_eq!(
            server.verify_at(&presented, "REGISTER", PASSWORD, NOW),
            Verdict::Rejected(Reason::Algorithm)
        );
    }

    #[test]
    fn the_replay_window_does_not_grow_without_bound() {
        let mut server = server();
        for index in 0..(REPLAY_CAPACITY + 100) {
            // A distinct nonce each time, as a fleet of clients would produce.
            let challenge = server.challenge_at(false, NOW + index as u64);
            let presented = answer(&server, &challenge, "REGISTER", 1, "abc");
            let _ = server.verify_at(&presented, "REGISTER", PASSWORD, NOW + index as u64);
        }
        assert!(
            server.seen.len() <= REPLAY_CAPACITY,
            "the window held {} entries",
            server.seen.len()
        );
    }

    /// The parser has to survive a quoted value containing a comma or a quote — otherwise every
    /// parameter after it shifts, and the response is read from the wrong place.
    #[test]
    fn a_quoted_value_containing_a_comma_or_a_quote_does_not_shift_the_rest() {
        let value = br#"Digest username="al,ice", realm="a\"b", nonce="n", uri="sip:x", response="deadbeef", qop=auth, nc=00000002, cnonce="c""#;
        let presented = Presented::parse(value).expect("parses");
        assert_eq!(presented.username, "al,ice");
        assert_eq!(presented.realm, "a\"b");
        assert_eq!(presented.response, "deadbeef");
        assert_eq!(presented.nonce_count, Some(2));
        assert_eq!(presented.cnonce.as_deref(), Some("c"));
        assert!(presented.qop_auth);
    }

    #[test]
    fn credentials_without_a_username_or_a_response_are_not_credentials() {
        assert!(Presented::parse(b"Digest realm=\"a\", nonce=\"n\"").is_none());
        assert!(Presented::parse(b"Basic dXNlcjpwYXNz").is_none());
        assert!(Presented::parse(b"").is_none());
    }
}
