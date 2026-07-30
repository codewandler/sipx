//! The TCP transport: framing across segment boundaries, connection reuse, and what a dropped
//! connection does to the transactions riding on it.
//!
//! Scenario numbers refer to `docs/specs/sip-transport.md` §11.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fmt::Write as _;
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, TransportKind, bind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::Receiver;

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

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

/// The whole stack over TCP, including the response travelling back over the same connection
/// (RFC 5923) rather than to whatever the `Via` claims.
#[tokio::test]
async fn a_tcp_request_gets_a_response_over_the_same_connection() {
    let (server, mut server_rx) = endpoint().await;
    let (client, _client_rx) = endpoint().await;
    let server_addr = server.local_addr();

    tokio::spawn(async move {
        let incoming = server_rx.recv().await.expect("a request");
        assert_eq!(incoming.transport, TransportKind::Tcp);
        server
            .respond(&incoming.key, ok_for(&incoming.request))
            .await
            .expect("responds");
    });

    let mut responses = client
        .send(
            options("tcp-basic@example.net"),
            Target::new(server_addr, TransportKind::Tcp),
        )
        .await
        .expect("sends");

    let response = tokio::time::timeout(Duration::from_secs(2), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response");
    assert_eq!(response.status.code(), 200);
}

/// X5: a message split across segments must be assembled. Framing on a stream is the parser's
/// job, and this is the case that catches a parser that assumes datagram boundaries.
#[tokio::test]
async fn a_message_split_across_segments_is_assembled() {
    let (server, mut server_rx) = endpoint().await;
    let server_addr = server.local_addr();

    let mut stream = tokio::net::TcpStream::connect(server_addr)
        .await
        .expect("connects");
    let text = "OPTIONS sip:a@b.com SIP/2.0\r\n\
         Via: SIP/2.0/TCP 127.0.0.1:5555;branch=z9hG4bKsplit\r\n\
         To: <sip:a@b.com>\r\n\
         From: <sip:c@d.net>;tag=1\r\n\
         Call-ID: split@example.net\r\n\
         CSeq: 1 OPTIONS\r\n\
         Max-Forwards: 70\r\n\
         Content-Length: 0\r\n\r\n";

    // One byte at a time: the most hostile segmentation a peer could produce.
    for byte in text.as_bytes() {
        stream.write_all(&[*byte]).await.expect("writes");
        stream.flush().await.expect("flushes");
    }

    let incoming = tokio::time::timeout(Duration::from_secs(2), server_rx.recv())
        .await
        .expect("no timeout")
        .expect("a request");
    assert_eq!(incoming.request.method, Method::Options);
    server
        .respond(&incoming.key, ok_for(&incoming.request))
        .await
        .expect("responds");

    let mut buf = vec![0u8; 4096];
    let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf))
        .await
        .expect("no timeout")
        .expect("reads");
    assert!(
        String::from_utf8_lossy(&buf[..read]).starts_with("SIP/2.0 200"),
        "the response comes back on the same connection"
    );
}

/// X6: two messages in one segment must both be delivered, in order.
#[tokio::test]
async fn two_messages_in_one_segment_are_both_delivered() {
    let (server, mut server_rx) = endpoint().await;
    let server_addr = server.local_addr();

    let mut stream = tokio::net::TcpStream::connect(server_addr)
        .await
        .expect("connects");

    let mut both = String::new();
    for branch in ["z9hG4bKfirst", "z9hG4bKsecond"] {
        write!(
            both,
            "OPTIONS sip:a@b.com SIP/2.0\r\n\
             Via: SIP/2.0/TCP 127.0.0.1:5555;branch={branch}\r\n\
             To: <sip:a@b.com>\r\n\
             From: <sip:c@d.net>;tag=1\r\n\
             Call-ID: {branch}@example.net\r\n\
             CSeq: 1 OPTIONS\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n"
        )
        .expect("writing to a String cannot fail");
    }
    stream.write_all(both.as_bytes()).await.expect("writes");

    let mut call_ids = Vec::new();
    for _ in 0..2 {
        let incoming = tokio::time::timeout(Duration::from_secs(2), server_rx.recv())
            .await
            .expect("no timeout")
            .expect("a request");
        call_ids.push(
            String::from_utf8_lossy(
                &incoming
                    .request
                    .headers
                    .value(&HeaderName::CallId)
                    .expect("a Call-ID"),
            )
            .into_owned(),
        );
        server
            .respond(&incoming.key, ok_for(&incoming.request))
            .await
            .expect("responds");
    }
    assert_eq!(
        call_ids,
        vec![
            "z9hG4bKfirst@example.net".to_owned(),
            "z9hG4bKsecond@example.net".to_owned()
        ],
        "both messages, in the order they were written"
    );
}

/// A body split from its headers, with a `Content-Length` that must be honoured exactly.
#[tokio::test]
async fn a_body_arriving_after_its_headers_is_framed_by_content_length() {
    let (server, mut server_rx) = endpoint().await;
    let mut stream = tokio::net::TcpStream::connect(server.local_addr())
        .await
        .expect("connects");

    let body = "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n";
    let headers = format!(
        "INVITE sip:a@b.com SIP/2.0\r\n\
         Via: SIP/2.0/TCP 127.0.0.1:5555;branch=z9hG4bKbody\r\n\
         To: <sip:a@b.com>\r\n\
         From: <sip:c@d.net>;tag=1\r\n\
         Call-ID: body@example.net\r\n\
         CSeq: 1 INVITE\r\n\
         Max-Forwards: 70\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {}\r\n\r\n",
        body.len()
    );

    stream.write_all(headers.as_bytes()).await.expect("writes");
    stream.flush().await.expect("flushes");
    tokio::time::sleep(Duration::from_millis(50)).await;
    stream.write_all(body.as_bytes()).await.expect("writes");

    let incoming = tokio::time::timeout(Duration::from_secs(2), server_rx.recv())
        .await
        .expect("no timeout")
        .expect("a request");
    assert_eq!(incoming.request.body().len(), body.len());
    assert_eq!(incoming.request.body().as_ref(), body.as_bytes());
}

/// X7: when the connection drops mid-transaction, the transaction must be told at once rather
/// than left to time out 32 seconds later.
#[tokio::test]
async fn a_dropped_connection_fails_its_transactions_immediately() {
    // A listener that accepts and then hangs up without answering.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let addr = listener.local_addr().expect("has an address");
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            // Read whatever arrives, then close.
            let mut stream = stream;
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            drop(stream);
        }
    });

    let (client, _rx) = endpoint().await;
    let mut responses = client
        .send(
            options("dropped@example.net"),
            Target::new(addr, TransportKind::Tcp),
        )
        .await
        .expect("sends");

    // Timer F would take 64*T1 = 32 seconds. Two is generous for "at once".
    let saw_error = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = responses.next().await {
            if matches!(event, sipx_sip::transaction::TuEvent::TransportError) {
                return true;
            }
        }
        false
    })
    .await
    .expect("the transaction must not wait for the 32-second timer");
    assert!(
        saw_error,
        "the transaction must be told the transport failed, not merely closed"
    );
}

/// A connection that fails to open at all is the same story, reached differently.
#[tokio::test]
async fn a_refused_connection_fails_its_transaction_promptly() {
    let (client, _rx) = endpoint().await;
    // Bind and immediately drop, so the port is almost certainly closed.
    let dead = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binds");
        listener.local_addr().expect("has an address")
    };

    let mut responses = client
        .send(
            options("refused@example.net"),
            Target::new(dead, TransportKind::Tcp),
        )
        .await
        .expect("sends");

    let saw_error = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = responses.next().await {
            if matches!(event, sipx_sip::transaction::TuEvent::TransportError) {
                return true;
            }
        }
        false
    })
    .await
    .expect("a refused connection must not hang the transaction");
    assert!(saw_error, "a refused connection is a transport error");
}

/// X17: endpoint shutdown owns its accepted connection tasks and closes quiet sockets.
#[tokio::test]
async fn endpoint_shutdown_closes_a_quiet_accepted_connection() {
    let (server, mut incoming) = endpoint().await;
    let address = server.local_addr();
    let mut peer = tokio::net::TcpStream::connect(address)
        .await
        .expect("connects");
    peer.write_all(
        b"OPTIONS sip:a@b.com SIP/2.0\r\n\
          Via: SIP/2.0/TCP 127.0.0.1:5555;branch=z9hG4bKshutdown\r\n\
          To: <sip:a@b.com>\r\n\
          From: <sip:c@d.net>;tag=1\r\n\
          Call-ID: shutdown@example.net\r\n\
          CSeq: 1 OPTIONS\r\n\
          Max-Forwards: 70\r\n\
          Content-Length: 0\r\n\r\n",
    )
    .await
    .expect("writes an admission probe");
    incoming.recv().await.expect("connection is admitted");

    let other_shutdown_caller = server.clone();
    tokio::join!(server.shutdown(), other_shutdown_caller.shutdown());

    // Returning from shutdown is the completion barrier, so no sleep or retry is needed before
    // observing the closed connection and reclaiming both transports on the endpoint's port.
    let mut byte = [0u8; 1];
    assert_eq!(peer.read(&mut byte).await.expect("read completes"), 0);
    let udp = tokio::net::UdpSocket::bind(address)
        .await
        .expect("UDP address is reusable when shutdown returns");
    let tcp = tokio::net::TcpListener::bind(address)
        .await
        .expect("TCP address is reusable when shutdown returns");
    drop((udp, tcp));
}

/// RFC 3261 §18.2.2: when the connection a request arrived on is gone, the response is sent by
/// opening a connection to the `received` address "using the port in the `sent-by` value, or
/// the default port for that transport". The source port is an ephemeral one the peer dialled
/// out from and is listening on for nothing; dialling it back loses the response even though
/// the peer is perfectly reachable at the port it advertised.
#[tokio::test]
async fn a_response_reconnects_to_the_sent_by_port_not_the_source_port() {
    let (server, mut server_rx) = endpoint().await;
    let server_addr = server.local_addr();

    // Where the peer says it can be reached — a port it really is listening on, which is not
    // the ephemeral port it dials out from.
    let advertised = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let advertised_port = advertised.local_addr().expect("has an address").port();

    let mut stream = tokio::net::TcpStream::connect(server_addr)
        .await
        .expect("connects");
    let text = format!(
        "OPTIONS sip:a@b.com SIP/2.0\r\n\
         Via: SIP/2.0/TCP 127.0.0.1:{advertised_port};branch=z9hG4bKreconnect\r\n\
         To: <sip:a@b.com>\r\n\
         From: <sip:c@d.net>;tag=1\r\n\
         Call-ID: reconnect@example.net\r\n\
         CSeq: 1 OPTIONS\r\n\
         Max-Forwards: 70\r\n\
         Content-Length: 0\r\n\r\n"
    );
    stream.write_all(text.as_bytes()).await.expect("writes");
    stream.flush().await.expect("flushes");

    let incoming = tokio::time::timeout(Duration::from_secs(2), server_rx.recv())
        .await
        .expect("no timeout")
        .expect("a request");

    // The peer goes away before the answer is ready, as a peer that has finished dialling out
    // routinely does.
    drop(stream);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let response = ok_for(&incoming.request);
    let _ = server.respond(&incoming.key, response).await;

    let (mut accepted, _) = tokio::time::timeout(Duration::from_secs(2), advertised.accept())
        .await
        .expect("the response must be sent to the advertised port")
        .expect("accepts");
    let mut buf = vec![0u8; 4096];
    let read = tokio::time::timeout(Duration::from_secs(2), accepted.read(&mut buf))
        .await
        .expect("no timeout")
        .expect("reads");
    assert!(
        String::from_utf8_lossy(&buf[..read]).starts_with("SIP/2.0 200"),
        "the reopened connection carries the response"
    );
}
