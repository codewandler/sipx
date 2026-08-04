//! Secure WebSocket: RFC 7118 framing over the certificate policy of `docs/specs/sip-tls.md`.
//!
//! The point of these is composition. WSS is not a third transport with its own security rules
//! — it is the TLS from `T-7` with the WebSocket from `T-8` on top, and what these assert is
//! that the policy really is the same one: a certificate that would be refused for `sips:` is
//! refused here, and refused *before* any SIP message crosses.

#![cfg(feature = "wss")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_testkit::certs::Ca;
use sipx_transport::tls::{ClientTls, Identity, ServerTls, TrustAnchors};
use sipx_transport::{Config, Handle, Incoming, Target, TransportKind, bind};
use tokio::sync::mpsc::Receiver;

// Above the ordinary stream ceiling so these messages only cross when `Config::limits` is
// propagated into the secure-WebSocket handshakes themselves, not merely into the SIP parser.
const CUSTOM_LIMIT_BODY: usize = 1024 * 1024 + 1024;

fn limits_above_default() -> sipx_sip::Limits {
    let mut limits = sipx_sip::Limits::stream();
    limits.max_message_bytes = CUSTOM_LIMIT_BODY + 4096;
    limits.max_body_bytes = CUSTOM_LIMIT_BODY;
    limits
}

fn trusting(ca: &Ca) -> TrustAnchors {
    let mut anchors = TrustAnchors::only();
    anchors.add_pem(ca.pem().as_bytes()).expect("a usable CA");
    anchors
}

/// An endpoint listening for secure WebSocket connections, presenting a certificate for `host`.
async fn wss_server(ca: &Ca, host: &str) -> (Handle, Receiver<Incoming>) {
    let (cert, key) = ca.issue_for(host);
    let identity = Identity::from_pem(cert.as_bytes(), key.as_bytes()).expect("an identity");
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.wss_server = Some((ServerTls::new(identity).expect("a server"), 0));
    bind(config).await.expect("binds")
}

/// A client that trusts this CA — an addition to its anchors, never a bypass.
async fn wss_client(ca: &Ca) -> (Handle, Receiver<Incoming>) {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.tls_client = Some(ClientTls::new(&trusting(ca)).expect("a client"));
    bind(config).await.expect("binds")
}

fn options() -> sipx_sip::Request {
    RequestBuilder::new(
        Method::Options,
        Uri::sip(Host::Name(HostName::new("localhost").expect("valid"))),
    )
    .header(HeaderName::To, "<sip:callee@localhost>")
    .expect("valid")
    .header(HeaderName::From, "<sip:caller@localhost>;tag=t1")
    .expect("valid")
    .header(HeaderName::CallId, Bytes::from_static(b"wss@localhost"))
    .expect("valid")
    .cseq(1, &Method::Options)
    .expect("valid")
    .max_forwards(70)
    .build()
}

/// The whole path: TLS handshake, upgrade, one message per frame, and the response back over
/// the same connection.
#[tokio::test]
async fn a_request_and_response_cross_wss() {
    let ca = Ca::new();
    let (server, mut server_rx) = wss_server(&ca, "localhost").await;
    let (client, _rx) = wss_client(&ca).await;

    let addr = server.wss_addr().expect("a WSS port was bound");
    let responder = tokio::spawn(async move {
        let incoming = server_rx.recv().await.expect("a request over WSS");
        assert_eq!(incoming.transport, TransportKind::Wss);
        let response = ResponseBuilder::to_request(
            &incoming.request,
            StatusCode::new(200).expect("valid"),
            "OK",
        )
        .expect("builds")
        .build();
        server
            .respond(&incoming.key, response)
            .await
            .expect("responds");
    });

    let target = Target::new(addr, TransportKind::Wss).verifying("localhost");
    let mut responses = client.send(options(), target).await.expect("sends");
    let response = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response over WSS");

    assert_eq!(response.status.code(), 200);
    responder.await.expect("the responder finishes");
}

/// X-64: the large request pins `Config::limits` at inbound WSS acceptance, while the large
/// response pins it at the outbound WSS connection. A default substituted at either endpoint
/// propagation site closes the connection at the WebSocket layer.
#[tokio::test]
async fn custom_endpoint_limits_reach_inbound_and_outbound_wss_handshakes() {
    let ca = Ca::new();
    let (cert, key) = ca.issue_for("localhost");
    let identity = Identity::from_pem(cert.as_bytes(), key.as_bytes()).expect("an identity");
    let mut server_config = Config::new("127.0.0.1:0".parse().expect("valid"));
    server_config.wss_server = Some((ServerTls::new(identity).expect("a server"), 0));
    server_config.limits = limits_above_default();
    let (server, mut server_rx) = bind(server_config).await.expect("server binds");
    let server_address = server.wss_addr().expect("a WSS port was bound");

    let mut client_config = Config::new("127.0.0.1:0".parse().expect("valid"));
    client_config.tls_client = Some(ClientTls::new(&trusting(&ca)).expect("a client"));
    client_config.limits = limits_above_default();
    let (client, _client_rx) = bind(client_config).await.expect("client binds");

    let responder = tokio::spawn(async move {
        let incoming = tokio::time::timeout(Duration::from_secs(5), server_rx.recv())
            .await
            .expect("custom inbound WSS limit admits the frame")
            .expect("a request over WSS");
        assert_eq!(incoming.request.body().len(), CUSTOM_LIMIT_BODY);
        let response = ResponseBuilder::to_request(
            &incoming.request,
            StatusCode::new(200).expect("valid"),
            "OK",
        )
        .expect("builds")
        .body(Bytes::from(vec![b'r'; CUSTOM_LIMIT_BODY]))
        .build();
        server
            .respond(&incoming.key, response)
            .await
            .expect("large response is sent");
    });

    let request = RequestBuilder::new(
        Method::Options,
        Uri::sip(Host::Name(HostName::new("localhost").expect("valid"))),
    )
    .header(HeaderName::To, "<sip:callee@localhost>")
    .expect("valid")
    .header(HeaderName::From, "<sip:caller@localhost>;tag=limits")
    .expect("valid")
    .header(HeaderName::CallId, "wss-limits@localhost")
    .expect("valid")
    .cseq(1, &Method::Options)
    .expect("valid")
    .max_forwards(70)
    .body(Bytes::from(vec![b'q'; CUSTOM_LIMIT_BODY]))
    .build();
    let target = Target::new(server_address, TransportKind::Wss).verifying("localhost");
    let mut responses = client
        .send(request, target)
        .await
        .expect("request is queued");
    let response = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("custom outbound WSS limit admits the frame")
        .expect("a final response over WSS");
    assert_eq!(response.body().len(), CUSTOM_LIMIT_BODY);
    responder.await.expect("responder finishes");
}

/// The policy is `T-7`'s because it is `T-7`'s code: a certificate for another host is refused
/// here exactly as it is for `sips:`, and the WebSocket upgrade never happens at all.
#[tokio::test]
async fn a_certificate_for_the_wrong_host_stops_the_upgrade() {
    let ca = Ca::new();
    let (server, mut server_rx) = wss_server(&ca, "elsewhere.example").await;
    let (client, _rx) = wss_client(&ca).await;

    let addr = server.wss_addr().expect("a WSS port");
    let target = Target::new(addr, TransportKind::Wss).verifying("localhost");

    let mut responses = client.send(options(), target).await.expect("queues");
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while responses.next().await.is_some() {}
    })
    .await;
    assert!(ended.is_ok(), "the transaction must end rather than hang");

    assert!(
        server_rx.try_recv().is_err(),
        "nothing may cross a connection that failed verification"
    );
}

/// And an issuer nobody vouches for is refused the same way. Note what is *not* on offer: there
/// is no path from here to an unencrypted `ws://` retry.
#[tokio::test]
async fn an_untrusted_issuer_is_refused_with_no_cleartext_fallback() {
    let ca = Ca::new();
    let stranger = Ca::new();
    let (server, mut server_rx) = wss_server(&ca, "localhost").await;
    let (client, _rx) = wss_client(&stranger).await;

    let addr = server.wss_addr().expect("a WSS port");
    let target = Target::new(addr, TransportKind::Wss).verifying("localhost");

    let mut responses = client.send(options(), target).await.expect("queues");
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while responses.next().await.is_some() {}
    })
    .await;
    assert!(ended.is_ok(), "the transaction must end");
    assert!(server_rx.try_recv().is_err(), "and nothing crossed");
}

/// An endpoint with no client configuration cannot open a WSS connection, and says so by name
/// rather than opening a plain one.
#[tokio::test]
async fn without_a_client_configuration_there_is_no_outbound_wss() {
    let ca = Ca::new();
    let (server, mut server_rx) = wss_server(&ca, "localhost").await;
    // No `tls_client`: nothing to verify against.
    let (client, _rx) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    let addr = server.wss_addr().expect("a WSS port");
    let target = Target::new(addr, TransportKind::Wss).verifying("localhost");

    let mut responses = client.send(options(), target).await.expect("queues");
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while responses.next().await.is_some() {}
    })
    .await;
    assert!(ended.is_ok(), "the transaction must end rather than hang");
    assert!(
        server_rx.try_recv().is_err(),
        "an unverifiable connection must not be opened anyway"
    );
}
