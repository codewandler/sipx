//! Two endpoints, real UDP sockets, real transactions.
//!
//! Scenario numbers refer to `docs/specs/sip-transport.md` §11.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::headers::Via;
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, TransportKind, bind};
use tokio::sync::mpsc::Receiver;

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    let config = Config::new("127.0.0.1:0".parse().expect("valid"));
    bind(config).await.expect("binds")
}

fn options_to(handle: &Handle) -> sipx_sip::Request {
    let uri = Uri::sip(Host::Name(HostName::new("example.com").expect("valid")));
    RequestBuilder::new(Method::Options, uri)
        .header(HeaderName::To, "<sip:callee@example.com>")
        .expect("valid")
        .header(HeaderName::From, "<sip:caller@example.net>;tag=t1")
        .expect("valid")
        .header(
            HeaderName::CallId,
            Bytes::from(format!("call-{}@example.net", handle.local_addr().port())),
        )
        .expect("valid")
        .cseq(1, &Method::Options)
        .expect("valid")
        .max_forwards(70)
        .build()
}

/// X1: the whole stack, end to end, over a real socket.
#[tokio::test]
async fn loopback_options_request_gets_200() {
    let (server, mut server_rx) = endpoint().await;
    let (client, _client_rx) = endpoint().await;

    let server_addr = server.local_addr();
    let responder = tokio::spawn(async move {
        let incoming = server_rx.recv().await.expect("a request arrives");
        assert_eq!(incoming.request.method, Method::Options);
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
        incoming
    });

    let mut responses = client
        .send(options_to(&client), Target::udp(server_addr))
        .await
        .expect("sends");

    let response = tokio::time::timeout(Duration::from_secs(2), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response");
    assert_eq!(response.status.code(), 200);

    let incoming = responder.await.expect("the responder finishes");
    assert_eq!(incoming.transport, TransportKind::Udp);
}

/// X2 and X3: the NAT machinery, observed from the far end. The client advertises the address
/// it is bound to and asks for `rport`; the server records what it actually saw.
#[tokio::test]
async fn the_server_records_received_and_rport() {
    let (server, mut server_rx) = endpoint().await;
    let (client, _client_rx) = endpoint().await;
    let client_port = client.local_addr().port();

    let mut responses = client
        .send(options_to(&client), Target::udp(server.local_addr()))
        .await
        .expect("sends");

    let incoming = tokio::time::timeout(Duration::from_secs(2), server_rx.recv())
        .await
        .expect("no timeout")
        .expect("a request");

    let via = incoming
        .request
        .headers
        .typed::<Via>()
        .expect("a Via")
        .expect("it parses");
    assert_eq!(
        via.rport().flatten().map(<[u8]>::to_vec),
        Some(client_port.to_string().into_bytes()),
        "rport must carry the port the server actually saw"
    );

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

    // And the response, sent to the observed address, arrives.
    let response = tokio::time::timeout(Duration::from_secs(2), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response");
    assert_eq!(response.status.code(), 200);
}

/// X4: one malformed packet must not disturb the socket. If it did, a single stray datagram
/// would be a denial of service.
#[tokio::test]
async fn a_malformed_datagram_does_not_break_the_endpoint() {
    let (server, mut server_rx) = endpoint().await;
    let (client, _client_rx) = endpoint().await;
    let server_addr = server.local_addr();

    let raw = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("binds");
    for junk in [
        &b"this is not a SIP message"[..],
        b"INVITE\r\n\r\n",
        b"\x00\x01\x02\x03",
        b"OPTIONS sip:a@b SIP/2.0\r\nContent-Length: -1\r\n\r\n",
    ] {
        raw.send_to(junk, server_addr).await.expect("sends");
    }

    // The endpoint is still there.
    let mut responses = client
        .send(options_to(&client), Target::udp(server_addr))
        .await
        .expect("sends");
    let incoming = tokio::time::timeout(Duration::from_secs(2), server_rx.recv())
        .await
        .expect("no timeout")
        .expect("a request still arrives");

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
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), responses.final_response())
            .await
            .expect("no timeout")
            .expect("a response")
            .status
            .code(),
        200
    );
}

/// A request retransmitted by the peer must reach the application exactly once. This is the
/// property the whole transaction layer exists for, verified here against a real socket
/// rather than in isolation.
#[tokio::test]
async fn a_retransmitted_request_reaches_the_application_once() {
    let (server, mut server_rx) = endpoint().await;
    let server_addr = server.local_addr();

    // Send the same datagram three times from a raw socket, as a peer that lost the response
    // would.
    let raw = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let text = "OPTIONS sip:a@b.com SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:5555;branch=z9hG4bKretransmit\r\n\
         To: <sip:a@b.com>\r\n\
         From: <sip:c@d.net>;tag=1\r\n\
         Call-ID: retransmit@example.net\r\n\
         CSeq: 1 OPTIONS\r\n\
         Max-Forwards: 70\r\n\
         Content-Length: 0\r\n\r\n";
    for _ in 0..3 {
        raw.send_to(text.as_bytes(), server_addr)
            .await
            .expect("sends");
    }

    let first = tokio::time::timeout(Duration::from_secs(2), server_rx.recv())
        .await
        .expect("no timeout")
        .expect("a request");
    assert_eq!(first.request.method, Method::Options);

    // Give the loop time to have delivered a duplicate if it were going to.
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        server_rx.try_recv().is_err(),
        "the application must see the request exactly once"
    );

    let response =
        ResponseBuilder::to_request(&first.request, StatusCode::new(200).expect("valid"), "OK")
            .expect("builds")
            .build();
    server
        .respond(&first.key, response)
        .await
        .expect("responds");
}

/// With nothing listening, the client transaction retransmits and eventually gives up. Run
/// with a compressed T1 so the test takes milliseconds rather than half a minute.
#[tokio::test]
async fn a_request_to_nowhere_times_out() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.timers = sipx_sip::Timers {
        t1: Duration::from_millis(5),
        t2: Duration::from_millis(20),
        t4: Duration::from_millis(20),
    };
    let (client, _rx) = bind(config).await.expect("binds");

    // A port nothing is bound to. The datagrams go nowhere; on loopback an ICMP rejection may
    // or may not surface, so the transaction is expected to end either way.
    let nowhere: std::net::SocketAddr = "127.0.0.1:9".parse().expect("valid");

    let mut responses = client
        .send(options_to(&client), Target::udp(nowhere))
        .await
        .expect("sends");

    let mut ended = false;
    while let Ok(event) = tokio::time::timeout(Duration::from_secs(3), responses.next()).await {
        if event.is_none() {
            ended = true;
            break;
        }
    }
    assert!(ended, "the transaction must end rather than hang");
}
