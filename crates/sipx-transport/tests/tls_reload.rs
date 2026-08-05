//! Live TLS/WSS server-identity rotation (`docs/specs/sip-tls.md` §3.6).

#![cfg(feature = "tls")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject as _;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_testkit::certs::Ca;
use sipx_transport::tls::{ClientTls, Identity, ServerTls, TrustAnchors, verification_name};
use sipx_transport::{Config, Handle, Target, TransportKind, bind};
use tokio::net::TcpStream;
use tokio::sync::Barrier;

fn trusting(cas: &[&Ca]) -> TrustAnchors {
    let mut anchors = TrustAnchors::only();
    for ca in cas {
        anchors
            .add_pem(ca.pem().as_bytes())
            .expect("fixture authority");
    }
    anchors
}

fn server_policy(ca: &Ca) -> (ServerTls, String, String) {
    let (certificate, key) = ca.issue_for("localhost");
    let identity = Identity::from_pem(certificate.as_bytes(), key.as_bytes()).expect("identity");
    (
        ServerTls::new(identity).expect("server policy"),
        certificate,
        key,
    )
}

fn replacement(certificate: &str, key: &str) -> Identity {
    Identity::from_pem(certificate.as_bytes(), key.as_bytes()).expect("parsed identity")
}

fn options(call_id: &'static [u8], cseq: u32) -> sipx_sip::Request {
    RequestBuilder::new(
        Method::Options,
        Uri::sip(Host::Name(HostName::new("localhost").expect("host"))),
    )
    .header(HeaderName::To, "<sip:callee@localhost>")
    .expect("To")
    .header(HeaderName::From, "<sip:caller@localhost>;tag=reload")
    .expect("From")
    .header(HeaderName::CallId, Bytes::from_static(call_id))
    .expect("Call-ID")
    .cseq(cseq, &Method::Options)
    .expect("CSeq")
    .max_forwards(70)
    .build()
}

async fn client(cas: &[&Ca]) -> Handle {
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.tls_client = Some(ClientTls::new(&trusting(cas)).expect("client TLS"));
    bind(config).await.expect("client binds").0
}

async fn exchange(
    client: &Handle,
    address: std::net::SocketAddr,
    transport: TransportKind,
    request: sipx_sip::Request,
) {
    let target = Target::new(address, transport).verifying("localhost");
    let mut responses = client.send(request, target).await.expect("request queues");
    let response = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("exchange is bounded")
        .expect("final response");
    assert_eq!(response.status.code(), 200);
}

async fn presented_leaf(
    address: std::net::SocketAddr,
    policy: ClientTls,
) -> CertificateDer<'static> {
    let tcp = TcpStream::connect(address).await.expect("TCP connects");
    let tls = policy
        .connector()
        .connect(verification_name("localhost").expect("server name"), tcp)
        .await
        .expect("TLS handshake");
    tls.get_ref()
        .1
        .peer_certificates()
        .and_then(|chain| chain.first())
        .cloned()
        .expect("server presented a leaf")
}

/// L11: validating the replacement happens before the publication point. A parsed identity may
/// still carry a certificate and key that do not belong together; refusing it must leave the old
/// server configuration selected for the next connection.
#[tokio::test]
async fn an_invalid_replacement_leaves_the_previous_identity_active() {
    let old_ca = Ca::new();
    let unrelated_ca = Ca::new();
    let (old_server, old_certificate, _old_key) = server_policy(&old_ca);
    let (_unrelated_server, _unrelated_certificate, unrelated_key) = server_policy(&unrelated_ca);

    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.tls_server = Some((old_server, 0));
    let (server, mut incoming) = bind(config).await.expect("server binds");
    let address = server.tls_addr().expect("TLS listener");

    let error = server
        .reload_server_identity(replacement(&old_certificate, &unrelated_key))
        .expect_err("mismatched key is refused");
    let rendered = error.to_string();
    assert!(rendered.contains("tls configuration"), "{rendered}");
    assert!(
        !rendered.contains(&unrelated_key),
        "private key leaked: {rendered}"
    );

    let responder = tokio::spawn(async move {
        let request = incoming.recv().await.expect("old identity still accepts");
        let response = ResponseBuilder::to_request(
            &request.request,
            StatusCode::new(200).expect("status"),
            "OK",
        )
        .expect("response")
        .build();
        server
            .respond(&request.key, response)
            .await
            .expect("responds");
    });
    let old_client = client(&[&old_ca]).await;
    exchange(
        &old_client,
        address,
        TransportKind::Tls,
        options(b"invalid-reload@localhost", 1),
    )
    .await;
    responder.await.expect("responder finishes");
}

/// L12: every concurrently selected configuration is one complete generation. The test reads the
/// actual leaf from each handshake, so a value other than the old or new certificate is observable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_handshakes_observe_only_a_complete_old_or_new_identity() {
    const HANDSHAKES: usize = 32;

    let old_ca = Ca::new();
    let new_ca = Ca::new();
    let (old_server, old_certificate, _old_key) = server_policy(&old_ca);
    let (_new_server, new_certificate, new_key) = server_policy(&new_ca);
    let old_der = CertificateDer::from_pem_slice(old_certificate.as_bytes()).expect("old DER");
    let new_der = CertificateDer::from_pem_slice(new_certificate.as_bytes()).expect("new DER");

    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.tls_server = Some((old_server, 0));
    config.handshake_limit = HANDSHAKES;
    let (server, _incoming) = bind(config).await.expect("server binds");
    let address = server.tls_addr().expect("TLS listener");
    let policy = ClientTls::new(&trusting(&[&old_ca, &new_ca])).expect("client policy");
    let barrier = Arc::new(Barrier::new(HANDSHAKES + 1));

    let mut handshakes = Vec::with_capacity(HANDSHAKES);
    for _ in 0..HANDSHAKES {
        let barrier = Arc::clone(&barrier);
        let policy = policy.clone();
        handshakes.push(tokio::spawn(async move {
            barrier.wait().await;
            presented_leaf(address, policy).await
        }));
    }

    barrier.wait().await;
    server
        .reload_server_identity(replacement(&new_certificate, &new_key))
        .expect("valid replacement");

    for handshake in handshakes {
        let leaf = handshake.await.expect("handshake task");
        assert!(
            leaf == old_der || leaf == new_der,
            "a handshake observed neither complete generation"
        );
    }

    assert_eq!(server.tls_addr(), Some(address), "reload does not rebind");
    assert_eq!(presented_leaf(address, policy).await, new_der);
}

/// L13: an established connection retains its handshake and remains pooled after publication. A
/// client that trusts only the old authority can send again only if no new handshake was attempted.
#[tokio::test]
async fn an_established_tls_connection_survives_identity_rotation() {
    let old_ca = Ca::new();
    let new_ca = Ca::new();
    let (old_server, _old_certificate, _old_key) = server_policy(&old_ca);
    let (_new_server, new_certificate, new_key) = server_policy(&new_ca);

    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.tls_server = Some((old_server, 0));
    let (server, mut incoming) = bind(config).await.expect("server binds");
    let address = server.tls_addr().expect("TLS listener");
    let server_for_responses = server.clone();
    let responder = tokio::spawn(async move {
        for _ in 0..3 {
            let request = incoming.recv().await.expect("request");
            let response = ResponseBuilder::to_request(
                &request.request,
                StatusCode::new(200).expect("status"),
                "OK",
            )
            .expect("response")
            .build();
            server_for_responses
                .respond(&request.key, response)
                .await
                .expect("responds");
        }
    });

    let old_client = client(&[&old_ca]).await;
    exchange(
        &old_client,
        address,
        TransportKind::Tls,
        options(b"old-flow-1@localhost", 1),
    )
    .await;
    server
        .reload_server_identity(replacement(&new_certificate, &new_key))
        .expect("valid replacement");
    exchange(
        &old_client,
        address,
        TransportKind::Tls,
        options(b"old-flow-2@localhost", 1),
    )
    .await;

    let new_client = client(&[&new_ca]).await;
    exchange(
        &new_client,
        address,
        TransportKind::Tls,
        options(b"new-flow@localhost", 1),
    )
    .await;
    responder.await.expect("responder finishes");
}

/// W15: WSS uses the same atomic TLS publication point. Its established WebSocket remains on the
/// old TLS stream, while a later connection selects the new identity before HTTP upgrade.
#[cfg(feature = "wss")]
#[tokio::test]
async fn an_established_wss_connection_survives_identity_rotation() {
    let old_ca = Ca::new();
    let new_ca = Ca::new();
    let (old_server, _old_certificate, _old_key) = server_policy(&old_ca);
    let (_new_server, new_certificate, new_key) = server_policy(&new_ca);

    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.wss_server = Some((old_server, 0));
    let (server, mut incoming) = bind(config).await.expect("server binds");
    let address = server.wss_addr().expect("WSS listener");
    let server_for_responses = server.clone();
    let responder = tokio::spawn(async move {
        for _ in 0..3 {
            let request = incoming.recv().await.expect("request");
            assert_eq!(request.transport, TransportKind::Wss);
            let response = ResponseBuilder::to_request(
                &request.request,
                StatusCode::new(200).expect("status"),
                "OK",
            )
            .expect("response")
            .build();
            server_for_responses
                .respond(&request.key, response)
                .await
                .expect("responds");
        }
    });

    let old_client = client(&[&old_ca]).await;
    exchange(
        &old_client,
        address,
        TransportKind::Wss,
        options(b"old-wss-1@localhost", 1),
    )
    .await;
    server
        .reload_server_identity(replacement(&new_certificate, &new_key))
        .expect("valid replacement");
    exchange(
        &old_client,
        address,
        TransportKind::Wss,
        options(b"old-wss-2@localhost", 1),
    )
    .await;

    let new_client = client(&[&new_ca]).await;
    exchange(
        &new_client,
        address,
        TransportKind::Wss,
        options(b"new-wss@localhost", 1),
    )
    .await;
    responder.await.expect("responder finishes");
    assert_eq!(server.wss_addr(), Some(address), "reload does not rebind");
}
