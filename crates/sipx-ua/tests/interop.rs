//! Interop against a real SIP server.
//!
//! These tests are `#[ignore]`d because they need a server running; `tests/interop/README.md`
//! says how to start one. They are the only tests here that prove sipx talks to something it
//! did not also write — every other test in this repo is sipx agreeing with itself, which is
//! exactly the kind of agreement that survives a wrong shared assumption.
//!
//! Run with:
//!
//! ```text
//! ./tests/interop/run.sh
//! ```

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::RequestBuilder;
use sipx_sip::{HeaderName, Host, HostName, Method, Uri};
use sipx_transport::{Config as TransportConfig, Target, TransportKind, bind};
use sipx_ua::{Config, Credentials, UserAgent};

/// Where the interop server listens. Overridable so the same tests can run against a server
/// on another host.
fn server() -> std::net::SocketAddr {
    std::env::var("SIPX_INTEROP_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:5060".to_owned())
        .parse()
        .expect("a valid address")
}

/// Where the server listens for TLS. Its own port, per RFC 3261 §19.1.2.
fn secure_server() -> std::net::SocketAddr {
    let mut addr = server();
    addr.set_port(
        std::env::var("SIPX_INTEROP_TLS_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(5061),
    );
    addr
}

/// The fixture authority the server's certificate was issued by.
///
/// Trusting it is an *addition* to the anchor set, never a bypass — there is no way to say
/// "accept anything", so a mistake in this harness produces a failed handshake rather than a
/// test that quietly proves nothing.
fn interop_anchors() -> sipx_transport::tls::TrustAnchors {
    let path = std::env::var("SIPX_INTEROP_CA").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/interop/kamailio/tls/ca.pem"
        )
        .to_owned()
    });
    let pem = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("{path}: {e}; run ./tests/interop/run.sh, which issues it"));
    let mut anchors = sipx_transport::tls::TrustAnchors::only();
    anchors.add_pem(&pem).expect("a usable fixture CA");
    anchors
}

async fn agent_over(transport: TransportKind) -> UserAgent {
    agent_to(Target::new(server(), transport), None).await
}

async fn agent_to(target: Target, anchors: Option<sipx_transport::tls::TrustAnchors>) -> UserAgent {
    let mut config = TransportConfig::new("127.0.0.1:0".parse().expect("valid"));
    if let Some(anchors) = anchors {
        config.tls_client = Some(sipx_transport::tls::ClientTls::new(&anchors).expect("a client"));
    }
    let (handle, _incoming) = bind(config).await.expect("binds");
    let registrar = Uri::sip(Host::Name(HostName::new("sipx.test").expect("valid")));
    let config = Config::new(
        "<sip:alice@sipx.test>",
        format!("<sip:alice@{}>", handle.local_addr()),
        registrar,
        target,
    )
    .with_credentials(Credentials::new("alice", "Circle Of Life"));
    UserAgent::new(handle, config)
}

/// M2's exit criterion over UDP: a real registrar challenges, sipx answers, the registrar
/// accepts and grants a lease.
#[tokio::test]
#[ignore = "needs a SIP server; see tests/interop/README.md"]
async fn registers_against_a_real_server_over_udp() {
    let mut ua = agent_over(TransportKind::Udp).await;
    let lease = tokio::time::timeout(Duration::from_secs(10), ua.register())
        .await
        .expect("no timeout")
        .expect("the registrar accepts our credentials");
    assert!(lease.granted > Duration::ZERO);
    assert!(lease.refresh_after < lease.granted);
}

/// The same over TCP, which exercises connection reuse for the response as well.
#[tokio::test]
#[ignore = "needs a SIP server; see tests/interop/README.md"]
async fn registers_against_a_real_server_over_tcp() {
    let mut ua = agent_over(TransportKind::Tcp).await;
    let lease = tokio::time::timeout(Duration::from_secs(10), ua.register())
        .await
        .expect("no timeout")
        .expect("the registrar accepts our credentials");
    assert!(lease.granted > Duration::ZERO);
}

/// A refresh must be accepted as a refresh. This is where a reused `CSeq` or a changed
/// `Call-ID` shows up against a real server and not against a permissive stub.
#[tokio::test]
#[ignore = "needs a SIP server; see tests/interop/README.md"]
async fn a_refresh_is_accepted_by_a_real_registrar() {
    let mut ua = agent_over(TransportKind::Udp).await;
    ua.register().await.expect("registers");
    let second = tokio::time::timeout(Duration::from_secs(10), ua.register())
        .await
        .expect("no timeout")
        .expect("the refresh is accepted");
    assert!(second.granted > Duration::ZERO);
}

/// A wrong password must be refused by the real server, not accepted. Without this, a broken
/// digest that happens to satisfy a permissive stub would look like success.
#[tokio::test]
#[ignore = "needs a SIP server; see tests/interop/README.md"]
async fn a_real_server_refuses_a_wrong_password() {
    let (handle, _incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let registrar = Uri::sip(Host::Name(HostName::new("sipx.test").expect("valid")));
    let config = Config::new(
        "<sip:alice@sipx.test>",
        format!("<sip:alice@{}>", handle.local_addr()),
        registrar,
        Target::udp(server()),
    )
    .with_credentials(Credentials::new("alice", "not the password"));
    let mut ua = UserAgent::new(handle, config);

    let result = tokio::time::timeout(Duration::from_secs(10), ua.register())
        .await
        .expect("no timeout");
    assert!(
        result.is_err(),
        "a real registrar must reject a wrong password: {result:?}"
    );
}

/// OPTIONS against a real element, answered by that element rather than by us.
#[tokio::test]
#[ignore = "needs a SIP server; see tests/interop/README.md"]
async fn a_real_server_answers_our_options_ping() {
    let (handle, _incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    let options = RequestBuilder::new(
        Method::Options,
        Uri::sip(Host::Name(HostName::new("sipx.test").expect("valid"))),
    )
    .header(HeaderName::To, "<sip:alice@sipx.test>")
    .expect("valid")
    .header(HeaderName::From, "<sip:sipx@example.net>;tag=probe")
    .expect("valid")
    .header(HeaderName::CallId, Bytes::from_static(b"interop-ping@sipx"))
    .expect("valid")
    .cseq(1, &Method::Options)
    .expect("valid")
    .max_forwards(70)
    .build();

    let mut responses = handle
        .send(options, Target::udp(server()))
        .await
        .expect("sends");
    let response = tokio::time::timeout(Duration::from_secs(10), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response");
    assert_eq!(response.status.code(), 200);
}

/// T-10, and the half of TLS that fixture tests cannot reach: another implementation agrees.
///
/// `T-7` proved sipx checks certificates the way the spec says against certificates sipx also
/// generated. This proves the handshake succeeds against a server that learned TLS from
/// somewhere else — and, because the digest exchange rides on top, that a challenge and its
/// answer survive the transport too.
#[tokio::test]
#[ignore = "needs a SIP server; see tests/interop/README.md"]
async fn registers_against_a_real_server_over_tls() {
    let target = Target::new(secure_server(), TransportKind::Tls).verifying("sipx.test");
    let mut ua = agent_to(target, Some(interop_anchors())).await;

    let lease = tokio::time::timeout(Duration::from_secs(15), ua.register())
        .await
        .expect("no timeout")
        .expect("the registrar accepts our credentials over TLS");
    assert!(lease.granted > Duration::ZERO);
}

/// And the negative, which is the one that matters. The server is genuine, its certificate is
/// signed by a CA we trust, and it is still not the name we set out to reach — so the
/// connection must not happen.
///
/// A stack that got this wrong would pass the test above and every fixture test in `T-7`.
#[tokio::test]
#[ignore = "needs a SIP server; see tests/interop/README.md"]
async fn refuses_a_real_server_presenting_the_wrong_name() {
    let target = Target::new(secure_server(), TransportKind::Tls).verifying("elsewhere.example");
    let mut ua = agent_to(target, Some(interop_anchors())).await;

    let started = std::time::Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(15), ua.register()).await;

    // Not merely "did not succeed": it must fail *now*. A failed handshake closes the
    // connection and every transaction on it, so the answer comes back in milliseconds. A test
    // that accepted a timeout would also pass if sipx had simply hung, or if the server were
    // not running at all.
    outcome
        .expect("verification failure is immediate, not a timeout")
        .expect_err("a certificate for another name must not be accepted");
    // The clock is the measurement here: the claim is *which* schedule ended the register, a
    // handshake rejection in milliseconds or the 15 s timeout above, and the only way to read that
    // is the elapsed time. Load can only push the number up, which is the direction that fails
    // (`X-44`).
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a refusal took {:?}; that is a timeout wearing a refusal's clothes",
        started.elapsed()
    );
}

/// And there is no way to make it accept one. Not a flag, not a builder method: trusting the
/// fixture CA is an addition to the anchors, so the only thing that could rescue the handshake
/// above is a certificate that actually names `elsewhere.example`.
#[tokio::test]
#[ignore = "needs a SIP server; see tests/interop/README.md"]
async fn a_real_server_is_refused_when_its_issuer_is_unknown() {
    let target = Target::new(secure_server(), TransportKind::Tls).verifying("sipx.test");
    // The platform's anchors, which do not vouch for a fixture CA.
    let mut ua = agent_to(target, Some(sipx_transport::tls::TrustAnchors::system())).await;

    let started = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(15), ua.register())
        .await
        .expect("verification failure is immediate, not a timeout")
        .expect_err("an unknown issuer must not be accepted");
    // The clock is the measurement, as in the test above: a refusal that took the whole timeout is
    // a hang wearing a refusal's clothes, and elapsed time is the only thing that tells them apart
    // (`X-44`).
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// Where the server serves SIP over WebSocket. RFC 7118 §5 fixes neither the port nor the
/// resource, so both are facts about the peer, declared in its profile: one peer serves it on
/// the SIP port at `/`, another from its own HTTP server on that server's port at `/ws`.
/// The defaults are the SIP port and the root, so a profile that says nothing gets the
/// arrangement every test in this file assumed before there was a choice to make.
fn ws_server() -> std::net::SocketAddr {
    let mut addr = server();
    if let Some(port) = std::env::var("SIPX_INTEROP_WS_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
    {
        addr.set_port(port);
    }
    addr
}

fn ws_path() -> String {
    std::env::var("SIPX_INTEROP_WS_PATH").unwrap_or_else(|_| "/".to_owned())
}

/// SIP over WebSocket against a real server's own WebSocket module.
///
/// Note the address: some peers serve WebSocket on the SIP port — the reason the connection
/// pool cannot be keyed by address alone — and some serve it from their HTTP server on its own
/// port, at a resource of their choosing. `T-23` is the story of the second kind.
#[tokio::test]
#[ignore = "needs a SIP server; see tests/interop/README.md"]
async fn registers_against_a_real_server_over_websocket() {
    let target = Target::new(ws_server(), TransportKind::Ws)
        .verifying("sipx.test")
        .at_path(ws_path());
    let mut ua = agent_to(target, None).await;

    let lease = tokio::time::timeout(Duration::from_secs(15), ua.register())
        .await
        .expect("no timeout")
        .expect("the registrar accepts our credentials over a websocket");
    assert!(lease.granted > Duration::ZERO);
}
