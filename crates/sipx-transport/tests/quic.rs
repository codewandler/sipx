//! SIP-over-QUIC vectors from `docs/specs/sip-quic.md` §8.

#![cfg(feature = "quic")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use bytes::Bytes;
use rustls_pki_types::pem::PemObject as _;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{
    HeaderName, Host, HostName, Limits, Message, Method, StatusCode, Uri, parse_datagram,
};
use sipx_testkit::certs::{Ca, dns};
use sipx_transport::tls::{ClientTls, Identity, ServerTls, TrustAnchors};
use sipx_transport::{Config, Target, TransportKind, bind};

fn trusting(ca: &Ca) -> TrustAnchors {
    let mut anchors = TrustAnchors::only();
    anchors.add_pem(ca.pem().as_bytes()).expect("a usable CA");
    anchors
}

fn options() -> sipx_sip::Request {
    options_with_body(Bytes::new())
}

fn options_with_body(body: Bytes) -> sipx_sip::Request {
    let uri = Uri::sip(Host::Name(HostName::new("localhost").expect("valid")));
    RequestBuilder::new(Method::Options, uri)
        .header(HeaderName::To, "<sip:callee@localhost>")
        .expect("valid")
        .header(HeaderName::From, "<sip:caller@localhost>;tag=q1")
        .expect("valid")
        .header(
            HeaderName::CallId,
            Bytes::from_static(b"quic-call@localhost"),
        )
        .expect("valid")
        .cseq(1, &Method::Options)
        .expect("valid")
        .max_forwards(70)
        .body(body)
        .build()
}

fn bare_client(ca: &Ca) -> quinn::Endpoint {
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    for certificate in CertificateDer::pem_slice_iter(ca.pem().as_bytes()) {
        roots.add(certificate.expect("certificate")).expect("root");
    }
    let mut tls = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"sip/2".to_vec()];
    tls.enable_early_data = false;
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).expect("QUIC TLS");
    let mut config = quinn::ClientConfig::new(std::sync::Arc::new(crypto));
    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_uni_streams(0_u8.into());
    config.transport_config(std::sync::Arc::new(transport));
    let mut endpoint =
        quinn::Endpoint::client("127.0.0.1:0".parse().expect("address")).expect("client endpoint");
    endpoint.set_default_client_config(config);
    endpoint
}

fn bare_server_with_alpn(ca: &Ca, alpn: Option<&[u8]>) -> quinn::Endpoint {
    let (cert, key) = ca.issue(&[dns("localhost")], "localhost");
    let certificates = CertificateDer::pem_slice_iter(cert.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("certificates");
    let private_key = PrivateKeyDer::from_pem_slice(key.as_bytes()).expect("private key");
    let mut tls = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certificates, private_key)
        .expect("server TLS");
    tls.alpn_protocols = alpn.into_iter().map(<[u8]>::to_vec).collect();
    tls.max_early_data_size = 0;
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls).expect("QUIC TLS");
    let config = quinn::ServerConfig::with_crypto(std::sync::Arc::new(crypto));
    quinn::Endpoint::server(config, "127.0.0.1:0".parse().expect("address"))
        .expect("server endpoint")
}

async fn bare_server(
    ca: &Ca,
) -> (
    sipx_transport::Handle,
    tokio::sync::mpsc::Receiver<sipx_transport::Incoming>,
) {
    let (cert, key) = ca.issue(&[dns("localhost")], "localhost");
    let identity = Identity::from_pem(cert.as_bytes(), key.as_bytes()).expect("identity");
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.quic_server = Some((ServerTls::new(identity).expect("server TLS"), 0));
    bind(config).await.expect("server binds")
}

const RAW_OPTIONS: &str = "OPTIONS sip:callee@localhost SIP/2.0\r\nVia: SIP/2.0/QUIC client.invalid;branch=z9hG4bKq-raw\r\nTo: <sip:callee@localhost>\r\nFrom: <sip:caller@localhost>;tag=q-raw\r\nCall-ID: q-raw@localhost\r\nCSeq: 1 OPTIONS\r\nMax-Forwards: 70\r\nContent-Length: 0\r\n\r\n";

async fn write_bare(connection: &quinn::Connection, bytes: &[u8]) {
    let (mut send, _recv) = connection.open_bi().await.expect("stream opens");
    send.write_all(bytes).await.expect("stream writes");
    send.finish().expect("stream finishes");
}

/// Q6, Q11 and Q12: one request stream carries provisional and final responses back.
#[tokio::test]
async fn one_message_crosses_a_quic_stream_and_its_response_returns_on_it() {
    let ca = Ca::new();
    let (cert, key) = ca.issue(&[dns("localhost")], "localhost");
    let identity = Identity::from_pem(cert.as_bytes(), key.as_bytes()).expect("identity");
    let mut server_config = Config::new("127.0.0.1:0".parse().expect("address"));
    server_config.quic_server = Some((ServerTls::new(identity).expect("server TLS"), 0));
    let (server, mut incoming) = bind(server_config).await.expect("server binds");

    let mut client_config = Config::new("127.0.0.1:0".parse().expect("address"));
    client_config.quic_client = Some(ClientTls::new(&trusting(&ca)).expect("client TLS"));
    let (client, _client_incoming) = bind(client_config).await.expect("client binds");

    let port = server.quic_addr().expect("QUIC listener").port();
    let target = Target::new(
        format!("127.0.0.1:{port}").parse().expect("target"),
        TransportKind::Quic,
    )
    .verifying("localhost");
    let answering = tokio::spawn(async move {
        let request = incoming.recv().await.expect("request");
        assert_eq!(request.transport, TransportKind::Quic);
        let trying = ResponseBuilder::to_request(
            &request.request,
            StatusCode::new(100).expect("status"),
            "Trying",
        )
        .expect("response")
        .build();
        server
            .respond(&request.key, trying)
            .await
            .expect("provisional response");
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

    let mut responses = client.send(options(), target).await.expect("sends");
    let provisional = tokio::time::timeout(Duration::from_secs(5), responses.next())
        .await
        .expect("response failure is bounded")
        .expect("provisional response");
    assert!(
        matches!(
            &provisional,
            sipx_sip::transaction::TuEvent::Response(response) if response.status.code() == 100
        ),
        "{provisional:?}"
    );
    let response = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("bounded by failure")
        .expect("final response");
    assert_eq!(response.status.code(), 200);
    answering.await.expect("answerer finishes");
}

/// Q2: the address is reachable but the authenticated name is wrong.
#[tokio::test]
async fn a_quic_certificate_for_the_wrong_host_is_refused() {
    let ca = Ca::new();
    let (cert, key) = ca.issue(&[dns("elsewhere.example")], "elsewhere.example");
    let identity = Identity::from_pem(cert.as_bytes(), key.as_bytes()).expect("identity");
    let mut server_config = Config::new("127.0.0.1:0".parse().expect("address"));
    server_config.quic_server = Some((ServerTls::new(identity).expect("server TLS"), 0));
    let (server, _incoming) = bind(server_config).await.expect("server binds");

    let mut client_config = Config::new("127.0.0.1:0".parse().expect("address"));
    client_config.quic_client = Some(ClientTls::new(&trusting(&ca)).expect("client TLS"));
    client_config.timers = sipx_sip::Timers {
        t1: Duration::from_millis(10),
        t2: Duration::from_millis(40),
        t4: Duration::from_millis(40),
    };
    let (client, _client_incoming) = bind(client_config).await.expect("client binds");
    let target = Target::new(
        server.quic_addr().expect("QUIC listener"),
        TransportKind::Quic,
    )
    .verifying("localhost");

    let mut responses = client
        .send(options(), target)
        .await
        .expect("transaction starts");
    let event = tokio::time::timeout(Duration::from_secs(5), responses.next())
        .await
        .expect("bounded by failure");
    assert!(matches!(
        event,
        Some(sipx_sip::transaction::TuEvent::TransportError)
    ));
    let detail = responses
        .take_transport_error()
        .expect("typed cause")
        .to_string();
    assert!(detail.contains("wrong host"), "{detail}");
}

/// Q1: a valid certificate from a CA outside the configured trust set is namedly refused.
#[tokio::test]
async fn a_quic_certificate_from_an_unknown_issuer_is_refused() {
    let server_ca = Ca::named("untrusted QUIC test CA");
    let trusted_ca = Ca::named("trusted QUIC test CA");
    let (server, _incoming) = bare_server(&server_ca).await;
    let mut client_config = Config::new("127.0.0.1:0".parse().expect("address"));
    client_config.quic_client = Some(ClientTls::new(&trusting(&trusted_ca)).expect("client TLS"));
    let (client, _client_incoming) = bind(client_config).await.expect("client binds");
    let target = Target::new(server.quic_addr().expect("listener"), TransportKind::Quic)
        .verifying("localhost");
    let mut responses = client
        .send(options(), target)
        .await
        .expect("transaction starts");
    let event = tokio::time::timeout(Duration::from_secs(5), responses.next())
        .await
        .expect("handshake failure is bounded");
    assert!(matches!(
        event,
        Some(sipx_sip::transaction::TuEvent::TransportError)
    ));
    let detail = responses
        .take_transport_error()
        .expect("typed failure")
        .to_string();
    assert!(detail.contains("unknown issuer"), "{detail}");
}

/// Q9 and Q12 together: the final response may use the stream end as its body delimiter even
/// when a Content-Length-framed provisional response preceded it on the same stream.
#[tokio::test]
async fn final_response_without_content_length_follows_a_provisional_response() {
    let ca = Ca::new();
    let peer = bare_server_with_alpn(&ca, Some(b"sip/2"));
    let addr = peer.local_addr().expect("server address");
    let (release, hold_open) = tokio::sync::oneshot::channel();
    let answering = tokio::spawn(async move {
        let connection = peer
            .accept()
            .await
            .expect("connection attempt")
            .await
            .expect("connects");
        let (mut send, mut recv) = connection.accept_bi().await.expect("request stream");
        let request = parse_datagram(
            Bytes::from(
                recv.read_to_end(Limits::stream().max_message_bytes)
                    .await
                    .expect("request bytes"),
            ),
            &Limits::stream(),
        )
        .expect("request parses");
        let Message::Request(request) = request else {
            panic!("client sent a response")
        };
        let trying = Message::Response(
            ResponseBuilder::to_request(&request, StatusCode::new(100).expect("status"), "Trying")
                .expect("response")
                .build(),
        )
        .to_bytes();
        let final_response = Message::Response(
            ResponseBuilder::to_request(&request, StatusCode::new(200).expect("status"), "OK")
                .expect("response")
                .build(),
        )
        .to_bytes();
        let final_response = String::from_utf8(final_response.to_vec())
            .expect("text fixture")
            .replace("Content-Length: 0\r\n", "");
        send.write_all(&trying).await.expect("provisional writes");
        send.write_all(final_response.as_bytes())
            .await
            .expect("final writes");
        send.finish().expect("stream finishes");
        hold_open.await.expect("client observed responses");
    });

    let mut client_config = Config::new("127.0.0.1:0".parse().expect("address"));
    client_config.quic_client = Some(ClientTls::new(&trusting(&ca)).expect("client TLS"));
    let (client, _incoming) = bind(client_config).await.expect("client binds");
    let target = Target::new(addr, TransportKind::Quic).verifying("localhost");
    let mut responses = client.send(options(), target).await.expect("sends");
    let provisional = tokio::time::timeout(Duration::from_secs(5), responses.next())
        .await
        .expect("response failure is bounded")
        .expect("provisional response");
    assert!(
        matches!(
            &provisional,
            sipx_sip::transaction::TuEvent::Response(response) if response.status.code() == 100
        ),
        "{provisional:?}"
    );
    let response = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("response failure is bounded")
        .expect("final response");
    assert_eq!(response.status.code(), 200);
    release.send(()).expect("bare peer is still running");
    answering.await.expect("bare peer finishes");
}

/// Q14: a closed connection fails its live transaction immediately with the close cause.
#[tokio::test]
async fn closing_a_quic_connection_fails_the_outstanding_transaction() {
    let ca = Ca::new();
    let peer = bare_server_with_alpn(&ca, Some(b"sip/2"));
    let addr = peer.local_addr().expect("server address");
    let (release, hold_open) = tokio::sync::oneshot::channel();
    let closing = tokio::spawn(async move {
        let connection = peer
            .accept()
            .await
            .expect("connection attempt")
            .await
            .expect("connects");
        let (_send, mut recv) = connection.accept_bi().await.expect("request stream");
        let _request = recv
            .read_to_end(Limits::stream().max_message_bytes)
            .await
            .expect("request bytes");
        connection.close(42_u8.into(), b"planned restart");
        hold_open.await.expect("client observed close");
    });
    let mut client_config = Config::new("127.0.0.1:0".parse().expect("address"));
    client_config.quic_client = Some(ClientTls::new(&trusting(&ca)).expect("client TLS"));
    let (client, _client_incoming) = bind(client_config).await.expect("client binds");
    let target = Target::new(addr, TransportKind::Quic).verifying("localhost");
    let mut responses = client
        .send(options(), target)
        .await
        .expect("transaction starts");
    let event = tokio::time::timeout(Duration::from_secs(5), responses.next())
        .await
        .expect("close failure is bounded");
    assert!(matches!(
        event,
        Some(sipx_sip::transaction::TuEvent::TransportError)
    ));
    let detail = responses
        .take_transport_error()
        .expect("typed close cause")
        .to_string();
    assert!(detail.contains("planned restart"), "{detail}");
    release.send(()).expect("bare peer is still running");
    closing.await.expect("bare peer finishes");
}

/// Q17: replacing the peer's UDP socket does not replace the authenticated QUIC connection.
#[tokio::test]
async fn a_peer_address_migration_keeps_the_connection_and_delivers_the_next_request() {
    let ca = Ca::new();
    let (server, mut incoming) = bare_server(&ca).await;
    let endpoint = bare_client(&ca);
    let original_addr = endpoint.local_addr().expect("client address");
    let connection = endpoint
        .connect(server.quic_addr().expect("listener"), "localhost")
        .expect("connect starts")
        .await
        .expect("connects");

    write_bare(&connection, RAW_OPTIONS.as_bytes()).await;
    let first = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
        .await
        .expect("first delivery is bounded")
        .expect("first request arrives");
    assert_eq!(first.request.method, Method::Options);

    let replacement = std::net::UdpSocket::bind("127.0.0.1:0").expect("replacement socket");
    let replacement_addr = replacement.local_addr().expect("replacement address");
    assert_ne!(original_addr, replacement_addr);
    endpoint.rebind(replacement).expect("endpoint migrates");

    let migrated = RAW_OPTIONS.replace("q-raw", "q-migrated");
    write_bare(&connection, migrated.as_bytes()).await;
    let second = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
        .await
        .expect("post-migration delivery is bounded")
        .expect("request arrives after migration");
    assert_eq!(second.request.method, Method::Options);
    assert_eq!(second.transport, TransportKind::Quic);
}

async fn alpn_refusal(offered: Option<&[u8]>) -> String {
    let ca = Ca::new();
    let server = bare_server_with_alpn(&ca, offered);
    let addr = server.local_addr().expect("server address");
    let accepting = tokio::spawn(async move {
        let incoming = server.accept().await.expect("connection attempt");
        incoming.await
    });
    let mut client_config = Config::new("127.0.0.1:0".parse().expect("address"));
    client_config.quic_client = Some(ClientTls::new(&trusting(&ca)).expect("client TLS"));
    let (client, _incoming) = bind(client_config).await.expect("client binds");
    let target = Target::new(addr, TransportKind::Quic).verifying("localhost");
    let mut responses = client
        .send(options(), target)
        .await
        .expect("transaction starts");
    let event = tokio::time::timeout(Duration::from_secs(5), responses.next())
        .await
        .expect("handshake failure is bounded");
    assert!(matches!(
        event,
        Some(sipx_sip::transaction::TuEvent::TransportError)
    ));
    let detail = responses
        .take_transport_error()
        .expect("typed failure")
        .to_string();
    let _ = accepting.await;
    detail
}

/// Q3: HTTP/3 is not SIP's registered application protocol.
#[tokio::test]
async fn a_peer_offering_h3_is_refused_for_wrong_alpn() {
    let detail = alpn_refusal(Some(b"h3")).await;
    assert!(detail.contains("wrong ALPN"), "{detail}");
}

/// Q4: no negotiated application protocol is also an absolute refusal.
#[tokio::test]
async fn a_peer_offering_no_alpn_is_refused() {
    let detail = alpn_refusal(None).await;
    assert!(detail.contains("wrong ALPN"), "{detail}");
}

/// Q5: `sip` is the WebSocket subprotocol, not QUIC's `sip/2` ALPN token.
#[tokio::test]
async fn a_peer_offering_the_websocket_sip_token_is_refused() {
    let detail = alpn_refusal(Some(b"sip")).await;
    assert!(detail.contains("wrong ALPN"), "{detail}");
}

/// Q7: the declared first message does not consume a stream that contains a second message.
#[tokio::test]
async fn two_messages_on_one_stream_close_the_connection() {
    let ca = Ca::new();
    let (server, mut incoming) = bare_server(&ca).await;
    let endpoint = bare_client(&ca);
    let connection = endpoint
        .connect(server.quic_addr().expect("listener"), "localhost")
        .expect("connect starts")
        .await
        .expect("connects");
    let twice = format!("{RAW_OPTIONS}{RAW_OPTIONS}");
    write_bare(&connection, twice.as_bytes()).await;
    let _closed = tokio::time::timeout(Duration::from_secs(5), connection.closed())
        .await
        .expect("connection failure is bounded");
    assert!(
        incoming.try_recv().is_err(),
        "malformed stream must not be delivered"
    );
}

/// Q9: without Content-Length, the body runs to the stream end.
#[tokio::test]
async fn a_message_without_content_length_is_accepted() {
    let ca = Ca::new();
    let (server, mut incoming) = bare_server(&ca).await;
    let endpoint = bare_client(&ca);
    let connection = endpoint
        .connect(server.quic_addr().expect("listener"), "localhost")
        .expect("connect starts")
        .await
        .expect("connects");
    let raw = RAW_OPTIONS.replace("Content-Length: 0\r\n", "");
    write_bare(&connection, raw.as_bytes()).await;
    let request = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
        .await
        .expect("delivery failure is bounded")
        .expect("request is delivered");
    assert_eq!(request.transport, TransportKind::Quic);
}

/// Q10: a declared body length must agree with the stream bytes exactly.
#[tokio::test]
async fn a_content_length_mismatch_closes_the_connection() {
    let ca = Ca::new();
    let (server, mut incoming) = bare_server(&ca).await;
    let endpoint = bare_client(&ca);
    let connection = endpoint
        .connect(server.quic_addr().expect("listener"), "localhost")
        .expect("connect starts")
        .await
        .expect("connects");
    let raw = RAW_OPTIONS.replace("Content-Length: 0\r\n\r\n", "Content-Length: 4\r\n\r\nabc");
    write_bare(&connection, raw.as_bytes()).await;
    let _closed = tokio::time::timeout(Duration::from_secs(5), connection.closed())
        .await
        .expect("connection failure is bounded");
    assert!(
        incoming.try_recv().is_err(),
        "mismatched stream must not be delivered"
    );
}

/// Q8: ending a stream before the SIP head is complete is malformed, not salvageable.
#[tokio::test]
async fn a_stream_ending_mid_message_closes_the_connection() {
    let ca = Ca::new();
    let (server, mut incoming) = bare_server(&ca).await;
    let endpoint = bare_client(&ca);
    let connection = endpoint
        .connect(server.quic_addr().expect("listener"), "localhost")
        .expect("connect starts")
        .await
        .expect("connects");
    write_bare(&connection, b"OPTIONS sip:callee@localhost SIP/2.0\r\nVia:").await;
    let _closed = tokio::time::timeout(Duration::from_secs(5), connection.closed())
        .await
        .expect("connection failure is bounded");
    assert!(
        incoming.try_recv().is_err(),
        "partial stream must not be delivered"
    );
}

/// Q19: a plain SIP datagram on the QUIC port is not answered.
#[tokio::test]
async fn a_plain_sip_datagram_on_the_quic_port_gets_no_response() {
    let ca = Ca::new();
    let (server, _incoming) = bare_server(&ca).await;
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("binds");
    socket
        .send_to(
            RAW_OPTIONS.as_bytes(),
            server.quic_addr().expect("listener"),
        )
        .await
        .expect("sends");
    let mut reply = [0_u8; 64];
    let received = tokio::time::timeout(Duration::from_millis(100), socket.recv(&mut reply)).await; // failure bound: how long the negative network assertion may run
    assert!(
        received.is_err(),
        "plain SIP must not create a QUIC response"
    );
}

/// Q20: an idle sipx connection emits a QUIC PING and no SIP-level request.
#[tokio::test(start_paused = true)]
async fn an_idle_connection_is_kept_alive_below_sip() {
    let ca = Ca::new();
    let (server, mut incoming) = bare_server(&ca).await;
    let endpoint = bare_client(&ca);
    let connection = endpoint
        .connect(server.quic_addr().expect("listener"), "localhost")
        .expect("connect starts")
        .await
        .expect("connects");
    let (mut send, mut recv) = connection.open_bi().await.expect("request stream");
    send.write_all(RAW_OPTIONS.as_bytes())
        .await
        .expect("request writes");
    send.finish().expect("request finishes");
    let request = incoming.recv().await.expect("request arrives");
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
    let _response = recv
        .read_to_end(Limits::stream().max_message_bytes)
        .await
        .expect("response bytes");

    let ping_before = connection.stats().frame_rx.ping;
    tokio::time::advance(Duration::from_secs(25)).await;
    let mut observed = false;
    for _ in 0..64 {
        if connection.stats().frame_rx.ping > ping_before {
            observed = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(observed, "the idle peer observed no QUIC PING frame");
    assert!(
        incoming.try_recv().is_err(),
        "the transport keepalive must not create a SIP request"
    );
}

/// Q23: QUIC packetises a message well above the datagram transport's MTU policy.
#[tokio::test]
async fn a_message_above_the_datagram_limit_is_delivered() {
    let ca = Ca::new();
    let (server, mut incoming) = bare_server(&ca).await;
    let mut client_config = Config::new("127.0.0.1:0".parse().expect("address"));
    client_config.quic_client = Some(ClientTls::new(&trusting(&ca)).expect("client TLS"));
    let (client, _client_incoming) = bind(client_config).await.expect("client binds");
    let target = Target::new(server.quic_addr().expect("listener"), TransportKind::Quic)
        .verifying("localhost");
    let request = options_with_body(Bytes::from(vec![b'x'; 4096]));
    let _responses = client.send(request, target).await.expect("sends");
    let request = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
        .await
        .expect("delivery failure is bounded")
        .expect("request arrives");
    assert_eq!(request.request.body().len(), 4096);
}
