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

async fn agent_over(transport: TransportKind) -> UserAgent {
    let (handle, _incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let registrar = Uri::sip(Host::Name(HostName::new("sipx.test").expect("valid")));
    let config = Config::new(
        "<sip:alice@sipx.test>",
        format!("<sip:alice@{}>", handle.local_addr()),
        registrar,
        Target::new(server(), transport),
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
