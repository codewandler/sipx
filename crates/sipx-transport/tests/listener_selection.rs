//! T-35: cleartext listener selection is exact, including the absence of UDP.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{CleartextTransports, Config, Error, Target, TransportKind, bind};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

fn options(call_id: &str) -> sipx_sip::Request {
    RequestBuilder::new(
        Method::Options,
        Uri::sip(Host::Name(HostName::new("example.test").expect("host"))),
    )
    .header(HeaderName::To, "<sip:callee@example.test>")
    .expect("To")
    .header(HeaderName::From, "<sip:caller@example.test>;tag=selection")
    .expect("From")
    .header(HeaderName::CallId, Bytes::from(call_id.to_owned()))
    .expect("Call-ID")
    .cseq(1, &Method::Options)
    .expect("CSeq")
    .max_forwards(70)
    .build()
}

async fn exchange(selection: CleartextTransports, transport: TransportKind, call_id: &str) {
    let mut server_config = Config::new("127.0.0.1:0".parse().expect("address"));
    server_config.cleartext = selection;
    let (server, mut incoming) = bind(server_config).await.expect("server binds");
    let mut client_config = Config::new("127.0.0.1:0".parse().expect("address"));
    client_config.cleartext = selection;
    let (client, _client_incoming) = bind(client_config).await.expect("client binds");

    let server_addr = server.local_addr();
    let answering = tokio::spawn(async move {
        let request = incoming.recv().await.expect("selected listener receives");
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
    let mut responses = client
        .send(options(call_id), Target::new(server_addr, transport))
        .await
        .expect("transaction starts");
    assert_eq!(
        responses
            .final_response()
            .await
            .expect("selected transport responds")
            .status
            .code(),
        200
    );
    answering.await.expect("answer task");
}

#[tokio::test]
async fn tcp_only_binds_no_udp_socket() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.cleartext = CleartextTransports::Tcp;
    let (endpoint, _incoming) = bind(config).await.expect("TCP-only endpoint binds");
    let address = endpoint.local_addr();

    let stream = TcpStream::connect(address)
        .await
        .expect("reported TCP address accepts connections");
    let udp = UdpSocket::bind(address)
        .await
        .expect("TCP-only endpoint did not reserve UDP");
    assert_eq!(endpoint.advertised(), address.to_string());

    drop((stream, udp));
    endpoint.shutdown().await;
}

#[tokio::test]
async fn udp_only_binds_no_tcp_listener() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.cleartext = CleartextTransports::Udp;
    let (endpoint, _incoming) = bind(config).await.expect("UDP-only endpoint binds");
    let address = endpoint.local_addr();

    let tcp = TcpListener::bind(address)
        .await
        .expect("UDP-only endpoint did not reserve TCP");
    assert!(
        UdpSocket::bind(address).await.is_err(),
        "the selected UDP socket is live"
    );

    drop(tcp);
    endpoint.shutdown().await;
}

#[tokio::test]
async fn udp_and_tcp_share_one_reported_address() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.cleartext = CleartextTransports::UdpAndTcp;
    let (endpoint, _incoming) = bind(config).await.expect("combined endpoint binds");
    let address = endpoint.local_addr();

    let stream = TcpStream::connect(address)
        .await
        .expect("TCP occupies the reported address");
    assert!(
        UdpSocket::bind(address).await.is_err(),
        "UDP occupies the same reported address"
    );

    drop(stream);
    endpoint.shutdown().await;
}

#[tokio::test]
async fn no_signalling_listener_is_a_pre_bind_error() {
    let address = "127.0.0.1:0".parse().expect("address");
    let mut config = Config::new(address);
    config.cleartext = CleartextTransports::None;
    let error = bind(config)
        .await
        .expect_err("an endpoint needs a listener");
    assert!(matches!(
        error,
        Error::InvalidConfig {
            field: "cleartext",
            ..
        }
    ));
}

#[tokio::test]
async fn each_selected_cleartext_transport_carries_a_transaction() {
    exchange(
        CleartextTransports::Udp,
        TransportKind::Udp,
        "udp-selection@example.test",
    )
    .await;
    exchange(
        CleartextTransports::Tcp,
        TransportKind::Tcp,
        "tcp-selection@example.test",
    )
    .await;
}

#[tokio::test]
async fn tcp_only_reports_an_outbound_udp_selection() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.cleartext = CleartextTransports::Tcp;
    let (endpoint, _incoming) = bind(config).await.expect("TCP-only endpoint binds");
    let destination = UdpSocket::bind("127.0.0.1:0").await.expect("destination");
    let mut responses = endpoint
        .send(
            options("missing-udp@example.test"),
            Target::udp(destination.local_addr().expect("destination address")),
        )
        .await
        .expect("the transaction reports asynchronous transport output");
    assert!(matches!(
        responses.next().await,
        Some(sipx_sip::transaction::TuEvent::TransportError)
    ));
    assert!(matches!(
        responses.take_transport_error(),
        Some(Error::TransportNotConfigured { transport: "UDP" })
    ));
    endpoint.shutdown().await;
}

#[cfg(feature = "ws")]
#[tokio::test]
async fn another_signalling_listener_allows_no_cleartext() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("address"));
    config.cleartext = CleartextTransports::None;
    config.ws_server = Some(0);
    let (endpoint, _incoming) = bind(config).await.expect("WebSocket-only endpoint binds");
    let ws_addr = endpoint.ws_addr().expect("WebSocket listener address");
    assert_eq!(endpoint.local_addr(), ws_addr);
    assert_eq!(endpoint.advertised(), ws_addr.to_string());
    UdpSocket::bind(ws_addr)
        .await
        .expect("WebSocket-only endpoint has no UDP placeholder");
    endpoint.shutdown().await;
}
