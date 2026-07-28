//! Registration (RFC 3261 §10): telling a registrar where to reach you, and keeping it told.
//!
//! A registration is not a request, it is a lease. The interesting parts are all about the
//! lease rather than the message:
//!
//! - The registrar decides the expiry, not the client. Asking for 3600 and being granted 60 is
//!   normal, and a client that refreshes on its own number instead of the granted one
//!   de-registers itself every time.
//! - The refresh has to happen *before* the lease ends, with enough margin to retry. sipx uses
//!   90% of the granted interval, floored so a very short lease still leaves room.
//! - `Call-ID` stays the same across refreshes and `CSeq` increases. A new `Call-ID` makes it a
//!   new registration rather than a refresh, which is how a client ends up with duplicate
//!   contacts at the registrar.
//! - A 401 or 407 is expected on the first attempt, not an error.

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::RequestBuilder;
use sipx_sip::{HeaderName, Method, Request, Response, Uri};

use crate::auth::{Challenge, Credentials, new_cnonce, respond, strongest};

/// How long a registration lease has left, and when to refresh it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    /// What the registrar granted.
    pub granted: Duration,
    /// When to refresh.
    pub refresh_after: Duration,
}

impl Lease {
    /// The refresh point for a granted interval.
    ///
    /// 90% of the lease, so a failed refresh still has time for the transaction to time out
    /// and be retried before the registration actually lapses. A refresh at 100% is a
    /// registration that lapses whenever a single packet is lost.
    #[must_use]
    pub fn from_granted(granted: Duration) -> Self {
        let seconds = granted.as_secs();
        let refresh = if seconds <= 20 {
            // Very short leases are used by test harnesses and some SBCs. Ten seconds of
            // margin does not fit in a 15-second lease, so fall back to half.
            seconds / 2
        } else {
            seconds * 9 / 10
        };
        Self {
            granted,
            refresh_after: Duration::from_secs(refresh.max(1)),
        }
    }
}

/// What a registration attempt produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Registered, with the lease the registrar granted.
    Registered(Lease),
    /// The registrar wants credentials. Answer with [`authorize`] and send again.
    Challenged(Box<Challenge>),
    /// The registrar refused.
    Rejected {
        /// The status code.
        status: u16,
        /// Its reason phrase.
        reason: String,
    },
}

/// What to register.
#[derive(Debug, Clone)]
pub struct Registration {
    /// The registrar's URI, which is the Request-URI of the REGISTER.
    pub registrar: Uri,
    /// The address of record being registered.
    pub aor: String,
    /// Where to reach this user agent.
    pub contact: String,
    /// How long a lease to ask for.
    pub expires: Duration,
    /// The `Call-ID`, constant across refreshes.
    pub call_id: String,
    /// The `CSeq`, increasing across refreshes.
    pub cseq: u32,
}

impl Registration {
    /// Build the REGISTER request.
    ///
    /// Note the two URIs that are easy to confuse: the Request-URI names the *registrar*, the
    /// `To` names the *user*. A REGISTER addressed to the user reaches nothing.
    pub fn request(&self) -> Result<Request, sipx_sip::error::BuildError> {
        Ok(
            RequestBuilder::new(Method::Register, self.registrar.clone())
                .header(HeaderName::To, Bytes::from(self.aor.clone()))?
                .header(
                    HeaderName::From,
                    Bytes::from(format!("{};tag={}", self.aor, new_cnonce())),
                )?
                .header(HeaderName::CallId, Bytes::from(self.call_id.clone()))?
                .cseq(self.cseq, &Method::Register)?
                .header(HeaderName::Contact, Bytes::from(self.contact.clone()))?
                .header(
                    HeaderName::Expires,
                    Bytes::from(self.expires.as_secs().to_string()),
                )?
                .max_forwards(70)
                .build(),
        )
    }

    /// Advance the sequence number for the next attempt.
    ///
    /// The `Call-ID` deliberately does not change: a new one makes this a new registration
    /// rather than a refresh, which leaves the old contact at the registrar until it expires.
    pub fn advance(&mut self) {
        self.cseq = self.cseq.saturating_add(1);
    }
}

/// Read what a registrar said.
#[must_use]
pub fn interpret(response: &Response, asked_for: Duration) -> Outcome {
    let status = response.status.code();

    if (200..300).contains(&status) {
        // The registrar's number wins. Refreshing on our own instead is how a client
        // de-registers itself on every cycle.
        let granted = granted_expiry(response).unwrap_or(asked_for);
        return Outcome::Registered(Lease::from_granted(granted));
    }

    if status == 401 || status == 407 {
        let from_proxy = status == 407;
        let header = if from_proxy {
            HeaderName::ProxyAuthenticate
        } else {
            HeaderName::WwwAuthenticate
        };
        let challenges: Vec<Challenge> = response
            .headers
            .get_all(&header)
            .filter_map(|h| Challenge::parse(&h.value(), from_proxy))
            .collect();
        if let Some(challenge) = strongest(challenges) {
            return Outcome::Challenged(Box::new(challenge));
        }
    }

    Outcome::Rejected {
        status,
        reason: String::from_utf8_lossy(&response.reason).into_owned(),
    }
}

/// The lease the registrar granted.
///
/// It may be in the `Contact` parameters or in `Expires`; the `Contact` wins, because it is
/// per-contact and the header is not.
fn granted_expiry(response: &Response) -> Option<Duration> {
    for header in response.headers.get_all(&HeaderName::Contact) {
        if let Some(seconds) = contact_expires(&header.value()) {
            return Some(Duration::from_secs(seconds));
        }
    }
    response
        .headers
        .value(&HeaderName::Expires)
        .and_then(|value| {
            std::str::from_utf8(&value)
                .ok()
                .and_then(|text| text.trim().parse::<u64>().ok())
        })
        .map(Duration::from_secs)
}

fn contact_expires(value: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(value).ok()?;
    for part in text.split(';').skip(1) {
        let mut halves = part.splitn(2, '=');
        let name = halves.next()?.trim();
        if name.eq_ignore_ascii_case("expires") {
            return halves.next()?.trim().trim_matches('"').parse().ok();
        }
    }
    None
}

/// Add credentials answering a challenge to a request.
pub fn authorize(
    request: &mut Request,
    challenge: &Challenge,
    credentials: &Credentials,
    nonce_count: u32,
) -> Result<(), sipx_sip::error::BuildError> {
    let uri = String::from_utf8_lossy(&request.uri.to_bytes()).into_owned();
    let method = String::from_utf8_lossy(request.method.as_bytes()).into_owned();
    let value = respond(
        challenge,
        credentials,
        &method,
        &uri,
        nonce_count,
        &new_cnonce(),
    );
    let header = sipx_sip::Header::build(challenge.response_header(), Bytes::from(value))?;
    request.headers.push(header);
    Ok(())
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
    use sipx_sip::{Host, HostName, Limits, Message, parse_datagram};

    fn registration() -> Registration {
        Registration {
            registrar: Uri::sip(Host::Name(HostName::new("example.com").expect("valid"))),
            aor: "<sip:alice@example.com>".to_owned(),
            contact: "<sip:alice@192.0.2.5:5060>".to_owned(),
            expires: Duration::from_secs(3600),
            call_id: "reg-1@192.0.2.5".to_owned(),
            cseq: 1,
        }
    }

    fn response(text: &str) -> Response {
        match parse_datagram(Bytes::from(text.to_owned()), &Limits::datagram()).expect("parses") {
            Message::Response(r) => r,
            Message::Request(_) => panic!("a response"),
        }
    }

    fn ok_with(extra: &str) -> Response {
        response(&format!(
            "SIP/2.0 200 OK\r\n\
             Via: SIP/2.0/UDP 192.0.2.5:5060;branch=z9hG4bKx\r\n\
             To: <sip:alice@example.com>;tag=r\r\n\
             From: <sip:alice@example.com>;tag=1\r\n\
             Call-ID: reg-1@192.0.2.5\r\n\
             CSeq: 1 REGISTER\r\n\
             {extra}\
             Content-Length: 0\r\n\r\n"
        ))
    }

    #[test]
    fn the_request_uri_names_the_registrar_and_the_to_names_the_user() {
        let request = registration().request().expect("builds");
        assert_eq!(request.uri.to_bytes().as_ref(), b"sip:example.com");
        assert_eq!(
            request
                .headers
                .value(&HeaderName::To)
                .expect("a To")
                .as_ref(),
            b"<sip:alice@example.com>"
        );
    }

    /// The registrar's number wins. A client that refreshes on the interval it *asked* for
    /// de-registers itself every cycle when the registrar grants less.
    #[test]
    fn the_granted_expiry_overrides_what_was_asked_for() {
        let outcome = interpret(
            &ok_with("Contact: <sip:alice@192.0.2.5:5060>;expires=60\r\n"),
            Duration::from_secs(3600),
        );
        match outcome {
            Outcome::Registered(lease) => assert_eq!(lease.granted, Duration::from_secs(60)),
            other => panic!("expected a lease, got {other:?}"),
        }
    }

    /// A per-contact expiry beats the `Expires` header, which applies to all of them.
    #[test]
    fn a_contact_expiry_beats_the_expires_header() {
        let outcome = interpret(
            &ok_with("Expires: 3600\r\nContact: <sip:alice@192.0.2.5:5060>;expires=120\r\n"),
            Duration::from_secs(3600),
        );
        match outcome {
            Outcome::Registered(lease) => assert_eq!(lease.granted, Duration::from_secs(120)),
            other => panic!("expected a lease, got {other:?}"),
        }
    }

    #[test]
    fn the_expires_header_is_used_when_the_contact_has_none() {
        let outcome = interpret(&ok_with("Expires: 300\r\n"), Duration::from_secs(3600));
        match outcome {
            Outcome::Registered(lease) => assert_eq!(lease.granted, Duration::from_secs(300)),
            other => panic!("expected a lease, got {other:?}"),
        }
    }

    /// The refresh must leave room to retry. Refreshing exactly at expiry means a single lost
    /// packet drops the registration.
    #[test]
    fn the_refresh_leaves_margin_before_the_lease_ends() {
        let lease = Lease::from_granted(Duration::from_secs(3600));
        assert_eq!(lease.refresh_after, Duration::from_secs(3240));
        assert!(lease.refresh_after < lease.granted);

        // And a short lease still leaves something.
        let short = Lease::from_granted(Duration::from_secs(15));
        assert!(short.refresh_after < short.granted);
        assert!(short.refresh_after >= Duration::from_secs(1));

        // Even a degenerate one-second lease must not schedule a refresh at zero, which would
        // spin.
        let degenerate = Lease::from_granted(Duration::from_secs(1));
        assert_eq!(degenerate.refresh_after, Duration::from_secs(1));
    }

    #[test]
    fn a_401_is_a_challenge_rather_than_a_failure() {
        let challenged = response(
            "SIP/2.0 401 Unauthorized\r\n\
             Via: SIP/2.0/UDP 192.0.2.5:5060;branch=z9hG4bKx\r\n\
             To: <sip:alice@example.com>;tag=r\r\n\
             From: <sip:alice@example.com>;tag=1\r\n\
             Call-ID: reg-1@192.0.2.5\r\n\
             CSeq: 1 REGISTER\r\n\
             WWW-Authenticate: Digest realm=\"example.com\", nonce=\"abc\", qop=\"auth\"\r\n\
             Content-Length: 0\r\n\r\n",
        );
        match interpret(&challenged, Duration::from_secs(3600)) {
            Outcome::Challenged(challenge) => {
                assert_eq!(challenge.realm, "example.com");
                assert!(challenge.qop_auth);
                assert!(!challenge.from_proxy);
            }
            other => panic!("expected a challenge, got {other:?}"),
        }
    }

    #[test]
    fn a_407_is_a_proxy_challenge_and_answered_in_the_proxy_header() {
        let challenged = response(
            "SIP/2.0 407 Proxy Authentication Required\r\n\
             Via: SIP/2.0/UDP 192.0.2.5:5060;branch=z9hG4bKx\r\n\
             To: <sip:alice@example.com>;tag=r\r\n\
             From: <sip:alice@example.com>;tag=1\r\n\
             Call-ID: reg-1@192.0.2.5\r\n\
             CSeq: 1 REGISTER\r\n\
             Proxy-Authenticate: Digest realm=\"p\", nonce=\"n\"\r\n\
             Content-Length: 0\r\n\r\n",
        );
        match interpret(&challenged, Duration::from_secs(3600)) {
            Outcome::Challenged(challenge) => {
                assert!(challenge.from_proxy);
                assert_eq!(challenge.response_header(), HeaderName::ProxyAuthorization);
            }
            other => panic!("expected a challenge, got {other:?}"),
        }
    }

    #[test]
    fn a_403_is_a_rejection_not_a_challenge() {
        let refused = response(
            "SIP/2.0 403 Forbidden\r\n\
             Via: SIP/2.0/UDP 192.0.2.5:5060;branch=z9hG4bKx\r\n\
             To: <sip:alice@example.com>;tag=r\r\n\
             From: <sip:alice@example.com>;tag=1\r\n\
             Call-ID: reg-1@192.0.2.5\r\n\
             CSeq: 1 REGISTER\r\n\
             Content-Length: 0\r\n\r\n",
        );
        match interpret(&refused, Duration::from_secs(3600)) {
            Outcome::Rejected { status, reason } => {
                assert_eq!(status, 403);
                assert_eq!(reason, "Forbidden");
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    /// A 401 whose challenge cannot be parsed is a rejection, not a challenge to answer. The
    /// alternative is retrying forever against a header we do not understand.
    #[test]
    fn a_401_with_an_unusable_challenge_is_a_rejection() {
        let bad = response(
            "SIP/2.0 401 Unauthorized\r\n\
             Via: SIP/2.0/UDP 192.0.2.5:5060;branch=z9hG4bKx\r\n\
             To: <sip:alice@example.com>;tag=r\r\n\
             From: <sip:alice@example.com>;tag=1\r\n\
             Call-ID: reg-1@192.0.2.5\r\n\
             CSeq: 1 REGISTER\r\n\
             WWW-Authenticate: Basic realm=\"example.com\"\r\n\
             Content-Length: 0\r\n\r\n",
        );
        assert!(matches!(
            interpret(&bad, Duration::from_secs(3600)),
            Outcome::Rejected { status: 401, .. }
        ));
    }

    /// A refresh keeps the `Call-ID` and advances the `CSeq`. A new `Call-ID` would leave the
    /// old contact registered until it expired on its own.
    #[test]
    fn a_refresh_keeps_the_call_id_and_advances_the_cseq() {
        let mut registration = registration();
        let first = registration.request().expect("builds");
        registration.advance();
        let second = registration.request().expect("builds");

        assert_eq!(
            first.headers.value(&HeaderName::CallId),
            second.headers.value(&HeaderName::CallId),
        );
        assert_eq!(
            second
                .headers
                .value(&HeaderName::CSeq)
                .expect("a CSeq")
                .as_ref(),
            b"2 REGISTER"
        );
    }

    /// The credentials are computed over the Request-URI of the request they authorize.
    #[test]
    fn authorization_covers_the_request_uri() {
        let mut request = registration().request().expect("builds");
        let challenge = Challenge::parse(
            br#"Digest realm="example.com", nonce="abc", qop="auth""#,
            false,
        )
        .expect("parses");
        authorize(
            &mut request,
            &challenge,
            &Credentials::new("alice", "secret"),
            1,
        )
        .expect("authorizes");

        let header = request
            .headers
            .value(&HeaderName::Authorization)
            .expect("an Authorization");
        let text = String::from_utf8_lossy(&header);
        assert!(text.contains(r#"uri="sip:example.com""#), "{text}");
        assert!(text.contains(r#"username="alice""#), "{text}");
        assert!(text.contains("nc=00000001"), "{text}");
    }
}
