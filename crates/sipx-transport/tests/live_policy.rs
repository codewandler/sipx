//! T-32: live source admission, bounded observation and structurally narrow request policy.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{Header, HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{
    Config, ConnectionState, EndpointObservation, Handle, MessageDirection, RequestPolicy,
    RequestPolicyDecision, RequestPolicyRef, SourcePrefix, Target, TransactionClass, TransportKind,
    bind,
};
use tokio::io::AsyncReadExt;

const EVENT_BOUND: Duration = Duration::from_secs(2);

fn options(call_id: &str) -> sipx_sip::Request {
    let uri = Uri::sip(Host::Name(HostName::new("example.com").expect("valid")));
    RequestBuilder::new(Method::Options, uri)
        .header(HeaderName::To, "<sip:callee@example.com>")
        .expect("valid")
        .header(HeaderName::From, "<sip:caller@example.net>;tag=t1")
        .expect("valid")
        .header(HeaderName::CallId, Bytes::from(call_id.to_owned()))
        .expect("valid")
        .cseq(1, &Method::Options)
        .expect("valid")
        .max_forwards(70)
        .build()
}

fn ok_for(request: &sipx_sip::Request) -> sipx_sip::Response {
    ResponseBuilder::to_request(request, StatusCode::new(200).expect("valid"), "OK")
        .expect("builds")
        .build()
}

async fn until(what: &str, mut condition: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + EVENT_BOUND;
    while !condition().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::task::yield_now().await;
    }
}

/// X39: admission is before both STUN classification and SIP parsing.
#[tokio::test]
async fn a_refused_udp_source_never_reaches_the_parser() {
    let (server, _incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("binds");
    server
        .replace_source_admission(vec![SourcePrefix::address(
            "192.0.2.1".parse().expect("address"),
        )])
        .expect("policy fits configured bound");

    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("sender binds");
    sender
        .send_to(b"not SIP and deliberately malformed", server.local_addr())
        .await
        .expect("datagram sends");

    until("source refusal was not counted", async || {
        server
            .counters()
            .transport(TransportKind::Udp)
            .source_refusals
            == 1
    })
    .await;
    let udp = server.counters().transport(TransportKind::Udp);
    assert_eq!(udp.parse_failures, 0, "refused bytes reached the parser");
}

/// X40: a stream is refused before its first byte reaches framing or a higher handshake.
#[tokio::test]
async fn a_refused_connection_closes_before_stream_parsing() {
    let (server, _incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("binds");
    server
        .replace_source_admission(vec![SourcePrefix::address(
            "192.0.2.1".parse().expect("address"),
        )])
        .expect("policy fits configured bound");

    let mut stream = tokio::net::TcpStream::connect(server.local_addr())
        .await
        .expect("TCP accept occurs below source policy");
    let mut byte = [0u8; 1];
    let read = tokio::time::timeout(EVENT_BOUND, stream.read(&mut byte))
        .await
        .expect("refusal is bounded")
        .expect("read reports EOF");
    assert_eq!(read, 0, "refused connection remained usable");
    assert_eq!(
        server
            .counters()
            .transport(TransportKind::Tcp)
            .source_refusals,
        1
    );
}

/// X40's secure half: admission runs before the TLS acceptor sees any handshake bytes.
#[cfg(feature = "tls")]
#[tokio::test]
async fn a_refused_tls_source_closes_before_handshake() {
    use sipx_testkit::certs::Ca;
    use sipx_transport::tls::{Identity, ServerTls};

    let ca = Ca::new();
    let (certificate, key) = ca.issue_for("localhost");
    let identity = Identity::from_pem(certificate.as_bytes(), key.as_bytes()).expect("identity");
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.tls_server = Some((ServerTls::new(identity).expect("server TLS"), 0));
    let (server, _incoming) = bind(config).await.expect("binds");
    server
        .replace_source_admission(vec![SourcePrefix::address(
            "192.0.2.1".parse().expect("address"),
        )])
        .expect("policy fits configured bound");

    let mut stream = tokio::net::TcpStream::connect(server.tls_addr().expect("TLS listener"))
        .await
        .expect("TCP accept occurs below source policy");
    let mut byte = [0u8; 1];
    assert_eq!(
        tokio::time::timeout(EVENT_BOUND, stream.read(&mut byte))
            .await
            .expect("refusal is bounded")
            .expect("read reports EOF"),
        0
    );
    assert_eq!(
        server
            .counters()
            .transport(TransportKind::Tls)
            .source_refusals,
        1
    );
}

#[cfg(feature = "ws")]
#[tokio::test]
async fn a_refused_websocket_source_closes_before_http_upgrade() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.ws_server = Some(0);
    let (server, _incoming) = bind(config).await.expect("binds");
    server
        .replace_source_admission(vec![SourcePrefix::address(
            "192.0.2.1".parse().expect("address"),
        )])
        .expect("policy fits configured bound");

    assert_refused_stream(
        &server,
        server.ws_addr().expect("WS listener"),
        TransportKind::Ws,
    )
    .await;
}

#[cfg(feature = "wss")]
#[tokio::test]
async fn a_refused_secure_websocket_source_closes_before_tls() {
    use sipx_testkit::certs::Ca;
    use sipx_transport::tls::{Identity, ServerTls};

    let ca = Ca::new();
    let (certificate, key) = ca.issue_for("localhost");
    let identity = Identity::from_pem(certificate.as_bytes(), key.as_bytes()).expect("identity");
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.wss_server = Some((ServerTls::new(identity).expect("server TLS"), 0));
    let (server, _incoming) = bind(config).await.expect("binds");
    server
        .replace_source_admission(vec![SourcePrefix::address(
            "192.0.2.1".parse().expect("address"),
        )])
        .expect("policy fits configured bound");

    assert_refused_stream(
        &server,
        server.wss_addr().expect("WSS listener"),
        TransportKind::Wss,
    )
    .await;
}

async fn assert_refused_stream(
    server: &Handle,
    address: std::net::SocketAddr,
    kind: TransportKind,
) {
    let mut stream = tokio::net::TcpStream::connect(address)
        .await
        .expect("TCP accept occurs below source policy");
    let mut byte = [0u8; 1];
    assert_eq!(
        tokio::time::timeout(EVENT_BOUND, stream.read(&mut byte))
            .await
            .expect("refusal is bounded")
            .expect("read reports EOF"),
        0
    );
    assert_eq!(server.counters().transport(kind).source_refusals, 1);
}

#[cfg(feature = "quic")]
#[tokio::test]
async fn a_refused_quic_source_closes_before_handshake() {
    use rustls_pki_types::CertificateDer;
    use rustls_pki_types::pem::PemObject as _;
    use sipx_testkit::certs::{Ca, dns};
    use sipx_transport::tls::{Identity, ServerTls};

    let ca = Ca::new();
    let (certificate, key) = ca.issue(&[dns("localhost")], "localhost");
    let identity = Identity::from_pem(certificate.as_bytes(), key.as_bytes()).expect("identity");
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.quic_server = Some((ServerTls::new(identity).expect("server TLS"), 0));
    let (server, _incoming) = bind(config).await.expect("binds");
    server
        .replace_source_admission(vec![SourcePrefix::address(
            "192.0.2.1".parse().expect("address"),
        )])
        .expect("policy fits configured bound");

    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    for certificate in CertificateDer::pem_slice_iter(ca.pem().as_bytes()) {
        roots.add(certificate.expect("certificate")).expect("root");
    }
    let mut tls = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"sip/2".to_vec()];
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).expect("QUIC TLS");
    let mut endpoint =
        quinn::Endpoint::client("127.0.0.1:0".parse().expect("address")).expect("client");
    endpoint.set_default_client_config(quinn::ClientConfig::new(std::sync::Arc::new(crypto)));
    let connecting = endpoint
        .connect(server.quic_addr().expect("QUIC listener"), "localhost")
        .expect("connect starts");
    let result = tokio::time::timeout(EVENT_BOUND, connecting)
        .await
        .expect("refusal is bounded");
    assert!(result.is_err(), "refused QUIC handshake succeeded");
    assert_eq!(
        server
            .counters()
            .transport(TransportKind::Quic)
            .source_refusals,
        1
    );
}

#[tokio::test]
async fn oversized_source_replacement_refuses_without_publishing() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.source_admission_limit = 1;
    let (server, mut incoming) = bind(config).await.expect("binds");
    let generation = server
        .replace_source_admission(vec![SourcePrefix::address(IpAddr::V4(Ipv4Addr::LOCALHOST))])
        .expect("first generation");
    let error = server
        .replace_source_admission(vec![
            SourcePrefix::address("192.0.2.1".parse().expect("address")),
            SourcePrefix::address("198.51.100.1".parse().expect("address")),
        ])
        .expect_err("oversized replacement");
    assert!(matches!(
        error,
        sipx_transport::Error::SourceAdmissionCapacity {
            max: 1,
            attempted: 2
        }
    ));

    let (sender, _sender_incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("sender binds");
    send_direct(&sender, server.local_addr(), "old-generation@example.net").await;
    let request = incoming
        .recv()
        .await
        .expect("old allow generation remains live");
    assert_eq!(request.transport, TransportKind::Udp);
    assert_eq!(generation, 1);
}

#[tokio::test]
async fn source_admission_bound_must_be_nonzero() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.source_admission_limit = 0;
    let error = bind(config).await.expect_err("zero work bound");
    assert!(matches!(
        error,
        sipx_transport::Error::InvalidConfig {
            field: "source_admission_limit",
            ..
        }
    ));
}

/// X41: replacement governs later accepts, not frames on a connection already admitted.
#[tokio::test]
async fn an_existing_connection_retains_its_admission_generation() {
    let (server, mut incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("server binds");
    let first_generation = server
        .replace_source_admission(vec![SourcePrefix::address(IpAddr::V4(Ipv4Addr::LOCALHOST))])
        .expect("policy fits configured bound");
    let (client, _client_incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("client binds");
    let target = Target::new(server.local_addr(), TransportKind::Tcp);

    let mut first = client
        .send(options("admission-one@example.net"), target.clone())
        .await
        .expect("first send");
    let request = incoming.recv().await.expect("first request");
    server
        .respond(&request.key, ok_for(&request.request))
        .await
        .expect("first response");
    first.final_response().await.expect("first final response");

    let second_generation = server
        .replace_source_admission(vec![SourcePrefix::address(
            "192.0.2.1".parse().expect("address"),
        )])
        .expect("policy fits configured bound");
    assert!(second_generation > first_generation);

    let mut second = client
        .send(options("admission-two@example.net"), target)
        .await
        .expect("existing connection remains eligible");
    let request = tokio::time::timeout(EVENT_BOUND, incoming.recv())
        .await
        .expect("existing generation remains live")
        .expect("second request");
    server
        .respond(&request.key, ok_for(&request.request))
        .await
        .expect("second response");
    second
        .final_response()
        .await
        .expect("second final response");

    let mut refused = tokio::net::TcpStream::connect(server.local_addr())
        .await
        .expect("new TCP reaches admission");
    let mut byte = [0u8; 1];
    assert_eq!(
        tokio::time::timeout(EVENT_BOUND, refused.read(&mut byte))
            .await
            .expect("new refusal is bounded")
            .expect("EOF"),
        0
    );
}

struct ProtectedPolicy;

impl RequestPolicy for ProtectedPolicy {
    fn decide(&self, _request: &sipx_sip::Request, _target: &Target) -> RequestPolicyDecision {
        RequestPolicyDecision::AddHeaders(vec![
            Header::build(
                HeaderName::CallId,
                Bytes::from_static(b"replacement@example.net"),
            )
            .expect("header"),
        ])
    }
}

struct SubjectPolicy;

impl RequestPolicy for SubjectPolicy {
    fn decide(&self, _request: &sipx_sip::Request, _target: &Target) -> RequestPolicyDecision {
        RequestPolicyDecision::AddHeaders(vec![
            Header::build(
                HeaderName::Subject,
                Bytes::from_static(b"application-owned"),
            )
            .expect("header"),
        ])
    }
}

#[derive(Clone)]
struct OneHeaderPolicy(HeaderName);

impl RequestPolicy for OneHeaderPolicy {
    fn decide(&self, _request: &sipx_sip::Request, _target: &Target) -> RequestPolicyDecision {
        RequestPolicyDecision::AddHeaders(vec![
            Header::build(self.0.clone(), Bytes::from_static(b"policy-value")).expect("header"),
        ])
    }
}

/// X38: policy gets no mutable message and stack-owned fields cannot return through its only output.
#[tokio::test]
async fn protected_policy_headers_are_refused_before_transaction_creation() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.request_policy = Some(RequestPolicyRef::new(ProtectedPolicy));
    let (sender, _incoming) = bind(config).await.expect("binds");
    let error = sender
        .send(
            options("original@example.net"),
            Target::udp("127.0.0.1:9".parse().expect("target")),
        )
        .await
        .expect_err("protected mutation must fail");
    assert!(
        matches!(error, sipx_transport::Error::ProtectedPolicyHeader { .. }),
        "{error:?}"
    );
    assert_eq!(sender.outstanding().await.expect("driver live"), 0);
}

#[tokio::test]
async fn policy_cannot_disguise_standard_or_protocol_semantic_headers() {
    let refused = [
        HeaderName::Other(Bytes::from_static(b"vIa")),
        HeaderName::Other(Bytes::from_static(b"v")),
        HeaderName::Contact,
        HeaderName::ContentType,
        HeaderName::Event,
    ];
    for name in refused {
        let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
        config.request_policy = Some(RequestPolicyRef::new(OneHeaderPolicy(name.clone())));
        let (sender, _incoming) = bind(config).await.expect("binds");
        let error = sender
            .send(
                options("protected-spelling@example.net"),
                Target::udp("127.0.0.1:9".parse().expect("target")),
            )
            .await
            .expect_err("policy field must be refused");
        assert!(
            matches!(error, sipx_transport::Error::ProtectedPolicyHeader { .. }),
            "{name:?}: {error:?}"
        );
        assert_eq!(sender.outstanding().await.expect("driver live"), 0);
    }
}

#[tokio::test]
async fn policy_cannot_append_a_duplicate_allowed_standard_header() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.request_policy = Some(RequestPolicyRef::new(SubjectPolicy));
    let (sender, _incoming) = bind(config).await.expect("binds");
    let mut request = options("duplicate-subject@example.net");
    request.headers.push(
        Header::build(HeaderName::Subject, Bytes::from_static(b"original")).expect("subject"),
    );
    let error = sender
        .send(request, Target::udp("127.0.0.1:9".parse().expect("target")))
        .await
        .expect_err("a second single-value field must be refused");
    assert!(matches!(
        error,
        sipx_transport::Error::ProtectedPolicyHeader { .. }
    ));
    assert_eq!(sender.outstanding().await.expect("driver live"), 0);
}

/// Allowed policy output lands before transport identity and the observer sees the finalized form.
#[tokio::test]
async fn finalized_outbound_observation_contains_policy_headers_and_via() {
    let (server, mut incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("server binds");
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.request_policy = Some(RequestPolicyRef::new(SubjectPolicy));
    let (sender, _sender_incoming) = bind(config).await.expect("sender binds");
    let mut observed = sender.observe(4);
    let responses = sender
        .send(
            options("finalized-policy@example.net"),
            Target::udp(server.local_addr()),
        )
        .await
        .expect("send");
    drop(responses);
    incoming.recv().await.expect("request arrives");

    let EndpointObservation::Message(event) = observed.recv().await.expect("outbound observation")
    else {
        panic!("UDP produces no connection event");
    };
    assert_eq!(event.direction, MessageDirection::Outbound);
    assert_eq!(event.transaction, TransactionClass::ClientCreated);
    let sipx_sip::Message::Request(request) = event.message else {
        panic!("observed outbound request");
    };
    assert!(request.headers.get(&HeaderName::Via).is_some());
    assert_eq!(
        request
            .headers
            .get(&HeaderName::Subject)
            .expect("policy subject")
            .value(),
        b"application-owned".as_slice()
    );
}

async fn send_direct(sender: &Handle, target: std::net::SocketAddr, call_id: &str) {
    let responses = sender
        .send(options(call_id), Target::udp(target))
        .await
        .expect("transaction send");
    drop(responses);
}

/// X36: a slow observer loses its own data, never endpoint progress.
#[tokio::test]
async fn observation_saturation_is_counted_without_blocking_the_driver() {
    let (server, mut incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("server binds");
    let mut observed = server.observe(1);
    let (sender, _sender_incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("sender binds");

    for suffix in 0..3 {
        send_direct(
            &sender,
            server.local_addr(),
            &format!("observation-{suffix}@example.net"),
        )
        .await;
    }
    for _ in 0..3 {
        tokio::time::timeout(EVENT_BOUND, incoming.recv())
            .await
            .expect("driver did not stall")
            .expect("request arrives");
    }
    until("observation overflow was not counted", async || {
        server.counters().observation_dropped >= 2
    })
    .await;
    let event = observed.recv().await.expect("one retained event");
    assert!(matches!(event, EndpointObservation::Message(_)));
}

/// X37: receiver closure detaches observation and leaves normal request delivery intact.
#[tokio::test]
async fn a_closed_observer_cannot_stop_network_processing() {
    let (server, mut incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("server binds");
    drop(server.observe(1));
    let (sender, _sender_incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("sender binds");
    send_direct(&sender, server.local_addr(), "closed-observer@example.net").await;
    let request = tokio::time::timeout(EVENT_BOUND, incoming.recv())
        .await
        .expect("closed observer did not block driver")
        .expect("request arrives");
    assert_eq!(request.request.method, Method::Options);
}

/// The public event carries parsed message, direction and transaction classification.
#[tokio::test]
async fn parsed_inbound_observation_carries_typed_context() {
    let (server, mut incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("server binds");
    let mut observed = server.observe(8);
    let (sender, _sender_incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("sender binds");
    send_direct(
        &sender,
        server.local_addr(),
        "typed-observation@example.net",
    )
    .await;
    let request = incoming.recv().await.expect("request arrives");
    let event = observed.recv().await.expect("observation arrives");
    let EndpointObservation::Message(event) = event else {
        panic!("first UDP event is the parsed message");
    };
    assert_eq!(event.direction, MessageDirection::Inbound);
    assert_eq!(event.transaction, TransactionClass::ServerCreated);
    assert_eq!(event.peer, request.source);
    assert_eq!(event.transport, TransportKind::Udp);
}

/// The same bounded stream names one connection incarnation through accept, open and pool entry.
#[tokio::test]
async fn connection_lifecycle_observation_has_a_stable_typed_identity() {
    let (server, mut incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("server binds");
    let mut observed = server.observe(16);
    let (sender, _sender_incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("sender binds");
    let responses = sender
        .send(
            options("connection-observation@example.net"),
            Target::new(server.local_addr(), TransportKind::Tcp),
        )
        .await
        .expect("send");
    drop(responses);
    incoming.recv().await.expect("request arrives");

    let mut identity = None;
    let mut states = Vec::new();
    while states.len() < 3 {
        let event = tokio::time::timeout(EVENT_BOUND, observed.recv())
            .await
            .expect("lifecycle is bounded")
            .expect("observer remains open");
        if let EndpointObservation::Connection(event) = event {
            if let Some(existing) = &identity {
                assert_eq!(
                    existing, &event.connection,
                    "one incarnation changed identity"
                );
            } else {
                assert!(event.connection.admission_generation.is_some());
                identity = Some(event.connection.clone());
            }
            states.push(event.state);
        }
    }
    assert_eq!(
        states,
        vec![
            ConnectionState::Accepted,
            ConnectionState::Opened,
            ConnectionState::Pooled,
        ]
    );
}
