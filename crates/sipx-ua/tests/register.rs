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

        granted_ok(&incoming.request, self.granted)
    }
}

/// A 200 that lists the bindings the way RFC 3261 §10.3 step 8 requires: the binding just
/// registered comes back as a `Contact` of its own, carrying the granted expiry.
fn granted_ok(request: &sipx_sip::Request, granted: u64) -> sipx_sip::Response {
    let contact = request
        .headers
        .value(&HeaderName::Contact)
        .map(|value| String::from_utf8_lossy(&value).into_owned())
        .expect("a REGISTER carries the contact it registers");
    ResponseBuilder::to_request(request, StatusCode::new(200).expect("valid"), "OK")
        .expect("builds")
        .header(
            HeaderName::Contact,
            Bytes::from(format!("{contact};expires={granted}")),
        )
        .expect("valid")
        .build()
}

/// How a counting registrar hands out nonces.
#[derive(Clone, Copy)]
enum NoncePolicy {
    /// A fresh nonce for every challenge.
    Fresh,
    /// The same nonce for every challenge.
    Fixed,
    /// The first answer is declared stale and re-challenged with a fresh nonce.
    StaleThenFresh,
}

/// A registrar that records the `nonce` and `nc` of every `Authorization` it receives.
///
/// It does not verify the digest — the verifying stub above covers that. What it checks is
/// the pairing RFC 7616 §3.4.3 defines: `nc` counts the requests sent *with this nonce*, so
/// what matters is which count arrived against which nonce.
async fn counting_registrar(
    policy: NoncePolicy,
) -> (Target, Arc<tokio::sync::Mutex<Vec<(String, String)>>>) {
    let (handle, mut incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let target = Target::udp(handle.local_addr());
    let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    tokio::spawn(async move {
        let mut issued = 0u32;
        while let Some(request) = incoming.recv().await {
            let authorization = request
                .request
                .headers
                .value(&HeaderName::Authorization)
                .map(|value| String::from_utf8_lossy(&value).into_owned());
            let response = match authorization {
                None => {
                    issued += 1;
                    let nonce = match policy {
                        NoncePolicy::Fixed => "fixed".to_owned(),
                        NoncePolicy::Fresh | NoncePolicy::StaleThenFresh => {
                            format!("nonce-{issued}")
                        }
                    };
                    challenge_with(&request.request, &nonce, false)
                }
                Some(authorization) => {
                    let nonce = param(&authorization, "nonce").unwrap_or_default();
                    let nc = param(&authorization, "nc").unwrap_or_default();
                    let mut seen = recorder.lock().await;
                    let first_answer = seen.is_empty();
                    seen.push((nonce, nc));
                    drop(seen);
                    if first_answer && matches!(policy, NoncePolicy::StaleThenFresh) {
                        issued += 1;
                        challenge_with(&request.request, &format!("nonce-{issued}"), true)
                    } else {
                        granted_ok(&request.request, 600)
                    }
                }
            };
            let _ = handle.respond(&request.key, response).await;
        }
    });
    (target, seen)
}

fn challenge_with(request: &sipx_sip::Request, nonce: &str, stale: bool) -> sipx_sip::Response {
    let stale = if stale { ", stale=true" } else { "" };
    ResponseBuilder::to_request(
        request,
        StatusCode::new(401).expect("valid"),
        "Unauthorized",
    )
    .expect("builds")
    .header(
        HeaderName::WwwAuthenticate,
        Bytes::from(format!(
            r#"Digest realm="{REALM}", nonce="{nonce}", qop="auth"{stale}"#
        )),
    )
    .expect("valid")
    .build()
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

/// S-36 / RFC 3261 §10.2.4: the registrar's granted expiry wins. A client that refreshed on the
/// interval it asked for would de-register itself every cycle.
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

/// S-36 / RFC 3261 §10.2.4: provisional responses do not decide a registration attempt. A
/// registrar is allowed to acknowledge the work with `100 Trying`; the binding is established by
/// the later final response.
#[tokio::test]
async fn a_hundred_trying_does_not_fail_registration() {
    let (registrar, mut incoming) =
        bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
            .await
            .expect("binds");
    let target = Target::udp(registrar.local_addr());
    let mut ua = agent(target, None).await;

    let registering = tokio::spawn(async move { ua.register().await });
    let request = incoming.recv().await.expect("REGISTER arrives");
    let trying = ResponseBuilder::to_request(
        &request.request,
        StatusCode::new(100).expect("valid"),
        "Trying",
    )
    .expect("builds")
    .build();
    registrar
        .respond(&request.key, trying)
        .await
        .expect("sends provisional response");
    registrar
        .respond(&request.key, granted_ok(&request.request, 60))
        .await
        .expect("sends final response");

    assert_eq!(
        registering
            .await
            .expect("registration task joins")
            .expect("the provisional response is ignored")
            .granted,
        Duration::from_secs(60)
    );
}

/// S-36 / RFC 3261 §10.2.4: one transient refresh failure must consume the safety margin the
/// granted lease reserved for retry, rather than immediately abandoning a binding that is still
/// valid at the registrar.
#[tokio::test(start_paused = true)]
async fn one_failed_refresh_is_retried_before_the_granted_lease_expires() {
    let (registrar, mut incoming) =
        bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
            .await
            .expect("binds");
    let target = Target::udp(registrar.local_addr());
    let mut ua = agent(target, None).await;
    let keeping = tokio::spawn(async move { ua.keep_registered().await });

    let initial = incoming.recv().await.expect("initial REGISTER arrives");
    registrar
        .respond(&initial.key, granted_ok(&initial.request, 100))
        .await
        .expect("grants initial lease");

    tokio::time::advance(Duration::from_secs(90)).await; // the clock is the measurement: assert the granted refresh deadline itself
    let refresh = incoming.recv().await.expect("refresh REGISTER arrives");
    let unavailable = ResponseBuilder::to_request(
        &refresh.request,
        StatusCode::new(503).expect("valid"),
        "Service Unavailable",
    )
    .expect("builds")
    .build();
    registrar
        .respond(&refresh.key, unavailable)
        .await
        .expect("refuses one refresh");

    let retry = tokio::time::timeout(Duration::from_secs(6), incoming.recv())
        .await
        .expect("a retry uses the remaining ten-second lease margin")
        .expect("the endpoint stays open");
    registrar
        .respond(&retry.key, granted_ok(&retry.request, 100))
        .await
        .expect("accepts retry");

    keeping.abort();
    let _ = keeping.await;
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

/// `P-25`: the bound is over the *attempt*, not over one transaction.
///
/// A registrar that challenges and then goes quiet costs two client transactions, and RFC 3261
/// §17.1.2.2 gives each of them 64·T1 of its own — so a caller who asked for five seconds and got
/// a per-transaction bound would be answered somewhere after sixty. The paused clock is the
/// measurement: nothing here waits on wall time, and the assertion is which of the two schedules
/// ended the attempt.
#[tokio::test(start_paused = true)]
async fn a_bounded_attempt_covers_the_authentication_retry() {
    let (registrar, mut incoming) =
        bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
            .await
            .expect("binds");
    let target = Target::udp(registrar.local_addr());
    let mut ua = agent(target, Some(Credentials::new(USERNAME, PASSWORD))).await;

    // The clone answers; the original stays in scope so the registrar's socket outlives the
    // attempt. A closed port would answer the client with ICMP rather than with silence, which
    // is a transport failure and a different test.
    let responder = registrar.clone();
    let serving = tokio::spawn(async move {
        let initial = incoming.recv().await.expect("the initial REGISTER arrives");
        responder
            .respond(
                &initial.key,
                challenge_with(&initial.request, "nonce-1", false),
            )
            .await
            .expect("challenges");
        incoming
            .recv()
            .await
            .expect("the authenticated retry arrives");
        // and is never answered: the retry's own schedule is the one this deadline pre-empts.
    });

    let started = tokio::time::Instant::now();
    let outcome = ua.register_within(Duration::from_secs(5)).await;
    let elapsed = started.elapsed();
    serving.await.expect("the registrar task joins");

    assert!(
        matches!(
            outcome,
            Err(sipx_ua::Error::AttemptTimeout { limit }) if limit == Duration::from_secs(5)
        ),
        "the attempt's own deadline is what expired: {outcome:?}"
    );
    assert!(
        elapsed >= Duration::from_secs(5),
        "the whole budget is spent before giving up: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(32),
        "gave up after {elapsed:?}, which is a transaction's schedule rather than the caller's"
    );
    assert!(
        ua.gruus().is_empty(),
        "an abandoned attempt claims no address for a binding it does not have"
    );
    assert!(
        ua.push_support().purr().is_none(),
        "nor anything the registrar said about a binding that was never recorded"
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

/// S-36 / RFC 3261 §22.4 and RFC 7616 §3.4.3: `nc` is "the count of the number of requests (including the current
/// request) that the client has sent with the nonce value in this request" — so the first
/// request under a given nonce carries `nc=00000001`. A stale challenge by definition
/// carries a fresh nonce, and a registrar tracking counts rejects a fresh nonce answered
/// with anything else as a replay.
#[tokio::test]
async fn a_stale_challenge_is_answered_with_a_count_of_one_for_the_fresh_nonce() {
    let (target, seen) = counting_registrar(NoncePolicy::StaleThenFresh).await;
    let mut ua = agent(target, Some(Credentials::new(USERNAME, PASSWORD))).await;

    tokio::time::timeout(Duration::from_secs(5), ua.register())
        .await
        .expect("no timeout")
        .expect("registers");

    let seen = seen.lock().await.clone();
    assert_eq!(
        seen,
        vec![
            ("nonce-1".to_owned(), "00000001".to_owned()),
            ("nonce-2".to_owned(), "00000001".to_owned()),
        ],
        "each nonce starts its own count at one"
    );
}

/// The count must not be carried across refreshes either: a registrar that challenges every
/// REGISTER with a fresh nonce sees `nc=00000001` every time.
#[tokio::test]
async fn a_fresh_nonce_on_a_refresh_restarts_the_count() {
    let (target, seen) = counting_registrar(NoncePolicy::Fresh).await;
    let mut ua = agent(target, Some(Credentials::new(USERNAME, PASSWORD))).await;

    ua.register().await.expect("registers");
    ua.register().await.expect("refreshes");

    let seen = seen.lock().await.clone();
    assert_eq!(
        seen,
        vec![
            ("nonce-1".to_owned(), "00000001".to_owned()),
            ("nonce-2".to_owned(), "00000001".to_owned()),
        ],
        "a nonce never used before starts at one"
    );
}

/// And the other direction: a registrar that keeps issuing the *same* nonce must see the
/// count advance, or it will reject the repeat of `nc=00000001` as a replay.
#[tokio::test]
async fn a_reused_nonce_advances_the_count() {
    let (target, seen) = counting_registrar(NoncePolicy::Fixed).await;
    let mut ua = agent(target, Some(Credentials::new(USERNAME, PASSWORD))).await;

    ua.register().await.expect("registers");
    ua.register().await.expect("refreshes");

    let seen = seen.lock().await.clone();
    assert_eq!(
        seen,
        vec![
            ("fixed".to_owned(), "00000001".to_owned()),
            ("fixed".to_owned(), "00000002".to_owned()),
        ],
        "the second request under the same nonce is the second of its count"
    );
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

/// Stand up a user agent, ping it with an out-of-dialog OPTIONS, and return the answer.
async fn options_response() -> sipx_sip::Response {
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
    tokio::time::timeout(Duration::from_secs(2), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response")
}

/// M2's other half: an OPTIONS ping is answered, and the answer says what we can do. A 200
/// with an empty `Allow` is a wasted exchange — the peer asked and learned nothing.
#[tokio::test]
async fn an_options_ping_is_answered_with_our_capabilities() {
    let response = options_response().await;

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

/// RFC 3261 §8.2.6.2: the UAS MUST add a tag to the `To` of any response other than a 100
/// when the request arrived without one. An out-of-dialog OPTIONS always arrives without
/// one, so a 200 that echoes the `To` verbatim violates the MUST on every ping.
#[tokio::test]
async fn an_options_answer_carries_a_to_tag() {
    let response = options_response().await;

    let to = response.headers.value(&HeaderName::To).expect("a To");
    let to = sipx_sip::Address::parse(&to, "To").expect("a parseable To");
    let tag = to.tag().expect("a To tag (RFC 3261 §8.2.6.2)");
    assert!(!tag.is_empty(), "an empty tag identifies nothing");
}

/// A registrar that returns a `Service-Route` on the first registration and none on the next.
///
/// The second answer is the interesting one: RFC 3608 §6.1 says a 2xx with no `Service-Route`
/// *clears* whatever was stored, and "keep the last one we saw" is the natural mis-implementation.
async fn registrar_that_stops_dictating_a_route(route: &'static str) -> Target {
    let (handle, mut incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let target = Target::udp(handle.local_addr());
    tokio::spawn(async move {
        let mut answered = 0u32;
        while let Some(request) = incoming.recv().await {
            let mut builder = ResponseBuilder::to_request(
                &request.request,
                StatusCode::new(200).expect("valid"),
                "OK",
            )
            .expect("builds")
            .header(
                HeaderName::Contact,
                Bytes::from_static(b"<sip:alice@127.0.0.1:5060>;expires=3600"),
            )
            .expect("valid");
            if answered == 0 {
                builder = builder
                    .header(
                        HeaderName::ServiceRoute,
                        Bytes::from_static(route.as_bytes()),
                    )
                    .expect("valid");
            }
            answered += 1;
            let _ = handle.respond(&request.key, builder.build()).await;
        }
    });
    target
}

#[tokio::test]
async fn a_registrars_service_route_is_stored_and_then_cleared_when_it_stops_sending_one() {
    let target =
        registrar_that_stops_dictating_a_route("<sip:edge.sipx.test;lr>, <sip:core.sipx.test;lr>")
            .await;
    let mut ua = agent(target, None).await;

    ua.register().await.expect("registers");
    assert_eq!(
        ua.service_route().rendered(),
        vec![
            "<sip:edge.sipx.test;lr>".to_owned(),
            "<sip:core.sipx.test;lr>".to_owned()
        ],
        "the service route the registrar dictated was not stored, or lost its order"
    );

    // The refresh carries no Service-Route. §6.1: "the UA clears any service route for that
    // address-of-record previously stored". Keeping it would route every outbound request
    // through a proxy the registrar has stopped naming — which fails as a 403 or a timeout at
    // the proxy, nowhere near the registration that caused it.
    ua.register().await.expect("refreshes");
    assert!(
        ua.service_route().is_empty(),
        "a 2xx with no Service-Route must clear the stored one, not leave it in place: {:?}",
        ua.service_route().rendered()
    );
}
