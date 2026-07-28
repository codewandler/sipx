//! Two endpoints, real UDP sockets, real transactions.
//!
//! Scenario numbers refer to `docs/specs/sip-transport.md` §11.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_transport::resolve::{Naptr, Resolver, Srv};
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

/// RFC 3261 §18.1.1: a request approaching the path MTU must not go out as a datagram. sipx
/// refuses it by name rather than sending something that will be fragmented or truncated — a
/// truncated SIP message is a security problem, not a degraded one.
#[tokio::test]
async fn an_oversized_datagram_is_refused_rather_than_truncated() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.mtu = 500;
    let (client, _rx) = bind(config).await.expect("binds");

    let (server, mut server_rx) = endpoint().await;
    let mut request = options_to(&client);
    // A body that puts the message comfortably over the limit.
    request.set_body(Bytes::from(vec![b'x'; 800]));

    let mut responses = client
        .send(request, Target::udp(server.local_addr()))
        .await
        .expect("the send is accepted; the failure surfaces on the transaction");

    let saw_error = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = responses.next().await {
            if matches!(event, sipx_sip::transaction::TuEvent::TransportError) {
                return true;
            }
        }
        false
    })
    .await
    .expect("no timeout");
    assert!(
        saw_error,
        "the transaction must be told, not left to time out"
    );

    // And nothing reached the far end.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        server_rx.try_recv().is_err(),
        "an oversized message must not be sent at all"
    );
}

struct TwoCandidates {
    live_port: u16,
}
impl Resolver for TwoCandidates {
    fn naptr(&self, _: &str) -> Vec<Naptr> {
        Vec::new()
    }
    fn srv(&self, name: &str) -> Vec<Srv> {
        if name.starts_with("_sip._udp.") {
            // Priority 1 is a port nothing listens on; priority 2 is the real endpoint.
            vec![
                Srv {
                    priority: 1,
                    weight: 0,
                    port: 9,
                    target: "dead.example".to_owned(),
                },
                Srv {
                    priority: 2,
                    weight: 0,
                    port: self.live_port,
                    target: "live.example".to_owned(),
                },
            ]
        } else {
            Vec::new()
        }
    }
    fn addresses(&self, _: &str) -> Vec<std::net::IpAddr> {
        vec!["127.0.0.1".parse().expect("valid")]
    }
}

/// T-4: a candidate that fails at the transport level is not the request failing. The first
/// candidate here is a black hole; the second is a live endpoint, and the exchange completes.
#[tokio::test]
async fn resolution_falls_through_to_the_next_candidate() {
    let (server, mut server_rx) = endpoint().await;
    let live_port = server.local_addr().port();

    // Compressed timers: a dead UDP candidate can only be detected by letting the transaction
    // time out, which with the default constants is 32 seconds.
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.timers = sipx_sip::Timers {
        t1: Duration::from_millis(5),
        t2: Duration::from_millis(20),
        t4: Duration::from_millis(20),
    };
    let (client, _rx) = bind(config).await.expect("binds");
    let uri = sipx_sip::Uri::sip(Host::Name(HostName::new("example.com").expect("valid")));

    let responder = tokio::spawn(async move {
        let incoming = server_rx
            .recv()
            .await
            .expect("a request reaches the live candidate");
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

    let mut responses = client
        .send_to_uri(options_to(&client), &uri, &TwoCandidates { live_port })
        .await
        .expect("at least one candidate works");

    let response = tokio::time::timeout(Duration::from_secs(3), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response");
    assert_eq!(response.status.code(), 200);
    responder.await.expect("the responder finishes");
}

/// UDP and TCP have independent port spaces, so a port the OS hands out for UDP may already be
/// held by someone else for TCP. An endpoint asking for "any port" must find one that is free
/// for both rather than failing — this surfaced as an intermittent `AddrInUse` under load,
/// which is exactly the kind of failure that gets blamed on the test.
#[tokio::test]
async fn binding_finds_a_port_free_for_both_transports() {
    // Hold a TCP port, then try to make the OS hand out the same number for UDP. The retry is
    // what makes this reliable rather than a coin toss.
    let squatter = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let taken = squatter.local_addr().expect("has an address").port();

    // Binding to that exact port must fail honestly: the caller named it, so the conflict is
    // real and not something to paper over by choosing another.
    let mut exact = Config::new(format!("127.0.0.1:{taken}").parse().expect("valid"));
    exact.tcp = true;
    assert!(
        bind(exact).await.is_err(),
        "a named port that is taken is a real conflict"
    );

    // Asking for any port must succeed, repeatedly, with both transports on the same number.
    for _ in 0..20 {
        let (handle, _rx) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
            .await
            .expect("an endpoint asking for any port must find one");
        let port = handle.local_addr().port();
        assert_ne!(port, 0);
        // Both transports really are on it: TCP would not have bound otherwise.
        assert_ne!(port, taken, "it must not have taken the held port");
    }
}

/// `respond` must mean the response is on the wire, not merely queued. The difference only
/// shows when a process answers and exits: a queued response is lost to the exit, and the
/// caller sees a timeout for a call that was in fact answered.
#[tokio::test]
async fn respond_returns_only_once_the_response_has_been_sent() {
    let (server, mut server_rx) = endpoint().await;

    // A raw socket standing in for the caller, so nothing else can be doing the receiving.
    let caller = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let caller_addr = caller.local_addr().expect("has an address");

    let text = format!(
        "OPTIONS sip:a@b.com SIP/2.0\r\n\
         Via: SIP/2.0/UDP {caller_addr};branch=z9hG4bKflush\r\n\
         To: <sip:a@b.com>\r\n\
         From: <sip:c@d.net>;tag=1\r\n\
         Call-ID: flush@example.net\r\n\
         CSeq: 1 OPTIONS\r\n\
         Max-Forwards: 70\r\n\
         Content-Length: 0\r\n\r\n"
    );
    caller
        .send_to(text.as_bytes(), server.local_addr())
        .await
        .expect("sends");

    let incoming = tokio::time::timeout(Duration::from_secs(2), server_rx.recv())
        .await
        .expect("no timeout")
        .expect("a request");

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

    // The moment `respond` returned, the datagram must already be readable. No sleep: if the
    // send were merely queued, this would time out.
    let mut buf = vec![0u8; 4096];
    let (len, _) = tokio::time::timeout(Duration::from_millis(50), caller.recv_from(&mut buf))
        .await
        .expect("the response is already on the wire when respond returns")
        .expect("receives");
    assert!(
        String::from_utf8_lossy(&buf[..len]).starts_with("SIP/2.0 200"),
        "expected the 200 we just sent"
    );
}

/// RFC 3261 §18.1.1's size limit is a rule for *sending requests*: a UAC that would exceed the
/// path MTU is told to use a congestion-controlled transport instead. §18.2.2 gives a UAS no
/// such choice — the response goes back per the topmost `Via`, over the transport the request
/// arrived on. Refusing to send it leaves the caller to time out while the callee believes it
/// answered, which a 200 carrying a full SDP answer reaches routinely.
#[tokio::test]
async fn an_oversized_response_is_sent_rather_than_refused() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.mtu = 500;
    let (server, mut server_rx) = bind(config).await.expect("binds");

    let (client, _client_rx) = endpoint().await;
    let request = options_to(&server);
    let server_addr = server.local_addr();

    tokio::spawn(async move {
        let incoming = server_rx.recv().await.expect("a request arrives");
        let mut response = ResponseBuilder::to_request(
            &incoming.request,
            StatusCode::new(200).expect("valid"),
            "OK",
        )
        .expect("builds")
        .build();
        response.set_body(Bytes::from(vec![b'x'; 800]));
        let _ = server.respond(&incoming.key, response).await;
    });

    let mut responses = client
        .send(request, Target::udp(server_addr))
        .await
        .expect("sends");

    let status = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = responses.next().await {
            if let sipx_sip::transaction::TuEvent::Response(response) = event {
                return Some(response.status.code());
            }
        }
        None
    })
    .await
    .expect("no timeout");
    assert_eq!(status, Some(200), "the response must reach the caller");
}

/// A request the application never answers must not pin a transaction for the life of the
/// process.
///
/// RFC 3261 §17.2 gives a server transaction in `Trying` no timer, because its model is that
/// the transaction user always responds. Real applications do not — one that ignores a method
/// it does not implement leaves the transaction there, and nothing collects it. A soak run
/// found 300 of them for 300 calls, still present two minutes later.
///
/// The clock is paused, so the three minutes cost no wall time. Asserting only that an
/// unanswered request *is* counted would pass with the whole backstop deleted; what has to be
/// asserted is that it eventually stops being counted.
#[tokio::test(start_paused = true)]
async fn a_request_the_application_never_answers_is_eventually_abandoned() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.unanswered_limit = Duration::from_secs(60);
    let (server, mut incoming) = bind(config).await.expect("binds");
    let server_addr = server.local_addr();
    let (client, _client_rx) = endpoint().await;

    assert_eq!(server.outstanding().await.expect("idle"), 0);

    let mut responses = client
        .send(options_to(&server), Target::udp(server_addr))
        .await
        .expect("sends");
    // Drained so the client transaction's own timers do not stall the paused clock.
    tokio::spawn(async move { while responses.next().await.is_some() {} });

    let request = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
        .await
        .expect("no timeout")
        .expect("a request arrives");

    let held = server.outstanding().await.expect("readable");
    assert!(
        held > 0,
        "an unanswered request must be visible as outstanding work"
    );

    // The client goes away first. Under a paused clock its retransmission timers fire whenever
    // the runtime idles, and a retransmission arriving after the sweep creates the server
    // transaction again — which would make this test a race rather than a measurement.
    client.shutdown().await;

    // Past the limit, and past the sweep interval that acts on it. The sweep and the command
    // are separate branches of the driver's `select!`, and which fires first is not ordered —
    // so the count is read until it settles rather than once.
    tokio::time::advance(Duration::from_secs(150)).await;
    let mut after = server.outstanding().await.expect("readable");
    for _ in 0..20 {
        if after == 0 {
            break;
        }
        tokio::time::advance(Duration::from_secs(31)).await;
        after = server.outstanding().await.expect("readable");
    }
    assert_eq!(
        after, 0,
        "a transaction nobody answered must not be held for the life of the process"
    );

    // And answering it now is refused rather than silently discarded: the application would
    // otherwise believe its response went out while the caller heard nothing at all.
    let response = sipx_sip::build::ResponseBuilder::to_request(
        &request.request,
        sipx_sip::StatusCode::new(200).expect("valid"),
        "OK",
    )
    .expect("builds")
    .build();
    let outcome = server.respond(&request.key, response).await;
    assert!(
        outcome.is_err(),
        "responding on an abandoned transaction must report that it is gone"
    );
}

/// A *provisional* response is not an answer. An application that sends 180 Ringing and then
/// wedges leaves a transaction in `Proceeding`, which RFC 3261 §17.2.1 gives no timer either —
/// so the backstop has to keep watching it. Exempting anything that got a response would exempt
/// exactly the calls most likely to be abandoned: the ones that rang.
#[tokio::test(start_paused = true)]
async fn a_transaction_that_only_ever_rang_is_still_abandoned() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.unanswered_limit = Duration::from_secs(60);
    let (server, mut incoming) = bind(config).await.expect("binds");
    let server_addr = server.local_addr();
    let (client, _client_rx) = endpoint().await;

    let mut responses = client
        .send(options_to(&server), Target::udp(server_addr))
        .await
        .expect("sends");
    tokio::spawn(async move { while responses.next().await.is_some() {} });

    let request = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
        .await
        .expect("no timeout")
        .expect("a request arrives");

    let ringing = sipx_sip::build::ResponseBuilder::to_request(
        &request.request,
        sipx_sip::StatusCode::new(180).expect("valid"),
        "Ringing",
    )
    .expect("builds")
    .build();
    server
        .respond(&request.key, ringing)
        .await
        .expect("the provisional goes out");

    assert!(
        server.outstanding().await.expect("readable") > 0,
        "it is still waiting on the application"
    );

    tokio::time::advance(Duration::from_secs(150)).await;
    let mut after = server.outstanding().await.expect("readable");
    for _ in 0..20 {
        if after == 0 {
            break;
        }
        tokio::time::advance(Duration::from_secs(31)).await;
        after = server.outstanding().await.expect("readable");
    }

    assert_eq!(
        after, 0,
        "a 180 is not an answer; the transaction must still be abandoned"
    );
}
