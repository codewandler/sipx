//! Registration end to end, against a registrar that actually verifies the digest.
//!
//! The registrar here is not a stub that returns 200: it issues a nonce, recomputes the
//! expected response from the password, and compares. A test whose server accepts anything
//! cannot tell a correct digest from an empty one.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use bytes::Bytes;
use md5::Md5;
use sha2::{Digest, Sha256};
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{HeaderName, Host, HostName, StatusCode, Uri};
use sipx_transport::{Config as TransportConfig, Handle, Incoming, Target, bind};
use sipx_ua::{Config, Credentials, UserAgent};
use tokio::sync::mpsc::Receiver;

const PASSWORD: &str = "Circle Of Life";
const USERNAME: &str = "alice";
const REALM: &str = "sipx.test";
const NONCE: &str = "dcd98b7102dd2f0e8b11d0f600bfb0c093";

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Recompute what the client should have sent, independently of the client's own code path.
fn expected_response(method: &str, uri: &str, nc: &str, cnonce: &str, sha256: bool) -> String {
    let hash = |input: &str| {
        if sha256 {
            hex(&Sha256::digest(input.as_bytes()))
        } else {
            hex(&Md5::digest(input.as_bytes()))
        }
    };
    let ha1 = hash(&format!("{USERNAME}:{REALM}:{PASSWORD}"));
    let ha2 = hash(&format!("{method}:{uri}"));
    hash(&format!("{ha1}:{NONCE}:{nc}:{cnonce}:auth:{ha2}"))
}

fn param(header: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let start = header.find(&needle)? + needle.len();
    let rest = header.get(start..)?;
    Some(if let Some(stripped) = rest.strip_prefix('"') {
        stripped.split('"').next()?.to_owned()
    } else {
        rest.split(',').next()?.trim().to_owned()
    })
}

/// A registrar that challenges once, then verifies the credentials it gets back.
struct Registrar {
    handle: Handle,
    granted: u64,
    sha256: bool,
    challenges: Arc<AtomicU32>,
}

impl Registrar {
    async fn serve(self, mut incoming: Receiver<Incoming>) {
        while let Some(request) = incoming.recv().await {
            let response = self.handle_one(&request);
            let _ = self.handle.respond(&request.key, response).await;
        }
    }

    fn handle_one(&self, incoming: &Incoming) -> sipx_sip::Response {
        let authorization = incoming
            .request
            .headers
            .value(&HeaderName::Authorization)
            .map(|value| String::from_utf8_lossy(&value).into_owned());

        let Some(authorization) = authorization else {
            self.challenges.fetch_add(1, Ordering::SeqCst);
            let algorithm = if self.sha256 { "SHA-256" } else { "MD5" };
            return ResponseBuilder::to_request(
                &incoming.request,
                StatusCode::new(401).expect("valid"),
                "Unauthorized",
            )
            .expect("builds")
            .header(
                HeaderName::WwwAuthenticate,
                Bytes::from(format!(
                    r#"Digest realm="{REALM}", nonce="{NONCE}", qop="auth", algorithm={algorithm}"#
                )),
            )
            .expect("valid")
            .build();
        };

        let nc = param(&authorization, "nc").unwrap_or_default();
        let cnonce = param(&authorization, "cnonce").unwrap_or_default();
        let got = param(&authorization, "response").unwrap_or_default();
        let uri = param(&authorization, "uri").unwrap_or_default();
        let want = expected_response("REGISTER", &uri, &nc, &cnonce, self.sha256);

        if got != want {
            return ResponseBuilder::to_request(
                &incoming.request,
                StatusCode::new(403).expect("valid"),
                "Forbidden",
            )
            .expect("builds")
            .build();
        }

        ResponseBuilder::to_request(
            &incoming.request,
            StatusCode::new(200).expect("valid"),
            "OK",
        )
        .expect("builds")
        .header(
            HeaderName::Contact,
            Bytes::from(format!(
                "<sip:alice@127.0.0.1:5060>;expires={}",
                self.granted
            )),
        )
        .expect("valid")
        .build()
    }
}

async fn registrar(granted: u64, sha256: bool) -> (Target, Arc<AtomicU32>) {
    let (handle, incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let target = Target::udp(handle.local_addr());
    let challenges = Arc::new(AtomicU32::new(0));
    tokio::spawn(
        Registrar {
            handle,
            granted,
            sha256,
            challenges: Arc::clone(&challenges),
        }
        .serve(incoming),
    );
    (target, challenges)
}

async fn agent(target: Target, credentials: Option<Credentials>) -> UserAgent {
    let (handle, _incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let uri = Uri::sip(Host::Name(HostName::new("sipx.test").expect("valid")));
    let mut config = Config::new(
        "<sip:alice@sipx.test>",
        format!("<sip:alice@{}>", handle.local_addr()),
        uri,
        target,
    );
    if let Some(credentials) = credentials {
        config = config.with_credentials(credentials);
    }
    UserAgent::new(handle, config)
}

/// The M2 exchange: REGISTER, 401 with a challenge, REGISTER with credentials, 200.
#[tokio::test]
async fn a_user_agent_registers_through_a_digest_challenge() {
    let (target, challenges) = registrar(3600, false).await;
    let mut ua = agent(target, Some(Credentials::new(USERNAME, PASSWORD))).await;

    let lease = tokio::time::timeout(Duration::from_secs(5), ua.register())
        .await
        .expect("no timeout")
        .expect("registers");

    assert_eq!(lease.granted, Duration::from_secs(3600));
    assert_eq!(
        challenges.load(Ordering::SeqCst),
        1,
        "exactly one challenge, then success"
    );
}

/// The same over SHA-256, which RFC 7616 prefers and which a modern registrar may insist on.
#[tokio::test]
async fn registration_works_over_sha256() {
    let (target, challenges) = registrar(600, true).await;
    let mut ua = agent(target, Some(Credentials::new(USERNAME, PASSWORD))).await;

    let lease = tokio::time::timeout(Duration::from_secs(5), ua.register())
        .await
        .expect("no timeout")
        .expect("registers");

    assert_eq!(lease.granted, Duration::from_secs(600));
    assert_eq!(challenges.load(Ordering::SeqCst), 1);
}

/// The registrar's number wins. A client that refreshed on the interval it asked for would
/// de-register itself every cycle.
#[tokio::test]
async fn the_granted_lease_is_shorter_than_the_one_requested() {
    let (target, _) = registrar(60, false).await;
    let mut ua = agent(target, Some(Credentials::new(USERNAME, PASSWORD))).await;

    let lease = ua.register().await.expect("registers");
    assert_eq!(lease.granted, Duration::from_secs(60));
    assert!(
        lease.refresh_after < lease.granted,
        "the refresh must leave margin: {lease:?}"
    );
}

/// A wrong password must fail once and stop, not loop. Looping is how a client locks out the
/// account it is trying to use.
#[tokio::test]
async fn a_wrong_password_fails_without_retrying_forever() {
    let (target, challenges) = registrar(3600, false).await;
    let mut ua = agent(target, Some(Credentials::new(USERNAME, "wrong"))).await;

    let result = tokio::time::timeout(Duration::from_secs(5), ua.register())
        .await
        .expect("no timeout");
    assert!(matches!(
        result,
        Err(sipx_ua::Error::Rejected { status: 403, .. })
    ));
    assert_eq!(
        challenges.load(Ordering::SeqCst),
        1,
        "one challenge, one answer, one rejection — no loop"
    );
}

/// Being challenged with no credentials configured is a distinct, named failure rather than a
/// generic one: the fix is different.
#[tokio::test]
async fn a_challenge_without_credentials_says_so() {
    let (target, _) = registrar(3600, false).await;
    let mut ua = agent(target, None).await;

    let result = tokio::time::timeout(Duration::from_secs(5), ua.register())
        .await
        .expect("no timeout");
    assert!(matches!(result, Err(sipx_ua::Error::CredentialsRequired)));
}

/// A refresh is the same registration, not a new one: same `Call-ID`, higher `CSeq`.
#[tokio::test]
async fn a_refresh_reuses_the_registration_rather_than_starting_another() {
    let (handle, mut incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let target = Target::udp(handle.local_addr());

    let seen = Arc::new(tokio::sync::Mutex::new(Vec::<(String, String)>::new()));
    let recorder = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Some(request) = incoming.recv().await {
            let call_id = String::from_utf8_lossy(
                &request
                    .request
                    .headers
                    .value(&HeaderName::CallId)
                    .expect("a Call-ID"),
            )
            .into_owned();
            let cseq = String::from_utf8_lossy(
                &request
                    .request
                    .headers
                    .value(&HeaderName::CSeq)
                    .expect("a CSeq"),
            )
            .into_owned();
            recorder.lock().await.push((call_id, cseq));

            let response = ResponseBuilder::to_request(
                &request.request,
                StatusCode::new(200).expect("valid"),
                "OK",
            )
            .expect("builds")
            .header(
                HeaderName::Contact,
                Bytes::from_static(b"<sip:alice@127.0.0.1:5060>;expires=3600"),
            )
            .expect("valid")
            .build();
            let _ = handle.respond(&request.key, response).await;
        }
    });

    let mut ua = agent(target, None).await;
    ua.register().await.expect("registers");
    // A refresh, as `keep_registered` would do it.
    ua.register().await.expect("refreshes");

    let exchanges = seen.lock().await.clone();
    assert_eq!(exchanges.len(), 2);
    assert_eq!(
        exchanges[0].0, exchanges[1].0,
        "a refresh keeps the Call-ID; a new one would leave the old contact registered"
    );
    assert_ne!(exchanges[0].1, exchanges[1].1, "and advances the CSeq");
}

/// M2's other half: an OPTIONS ping is answered, and the answer says what we can do. A 200
/// with an empty `Allow` is a wasted exchange — the peer asked and learned nothing.
#[tokio::test]
async fn an_options_ping_is_answered_with_our_capabilities() {
    let (handle, incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let agent_addr = handle.local_addr();
    let uri = Uri::sip(Host::Name(HostName::new("sipx.test").expect("valid")));
    let config = Config::new(
        "<sip:alice@sipx.test>",
        format!("<sip:alice@{agent_addr}>"),
        uri,
        Target::udp(agent_addr),
    );
    let ua = UserAgent::new(handle, config);

    let mut incoming = incoming;
    tokio::spawn(async move {
        while let Some(request) = incoming.recv().await {
            let _ = ua.answer(&request).await;
        }
    });

    let (caller, _rx) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let options = sipx_sip::build::RequestBuilder::new(
        sipx_sip::Method::Options,
        Uri::sip(Host::Name(HostName::new("sipx.test").expect("valid"))),
    )
    .header(HeaderName::To, "<sip:alice@sipx.test>")
    .expect("valid")
    .header(HeaderName::From, "<sip:probe@example.net>;tag=p1")
    .expect("valid")
    .header(HeaderName::CallId, Bytes::from_static(b"ping@example.net"))
    .expect("valid")
    .cseq(1, &sipx_sip::Method::Options)
    .expect("valid")
    .max_forwards(70)
    .build();

    let mut responses = caller
        .send(options, Target::udp(agent_addr))
        .await
        .expect("sends");
    let response = tokio::time::timeout(Duration::from_secs(2), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response");

    assert_eq!(response.status.code(), 200);
    let allow = response
        .headers
        .value(&HeaderName::Allow)
        .expect("OPTIONS must be answered with an Allow, or the exchange told the peer nothing");
    let allow = String::from_utf8_lossy(&allow);
    for method in ["INVITE", "ACK", "CANCEL", "BYE", "OPTIONS"] {
        assert!(allow.contains(method), "{method} missing from {allow}");
    }
}
