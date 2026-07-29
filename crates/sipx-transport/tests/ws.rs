//! SIP over WebSocket, against a peer that is not sipx.
//!
//! Vector numbers refer to `docs/specs/sip-tls.md` §6. The peer here is a bare WebSocket driven
//! by the test, which is the point: framing bugs hide when both ends share the same framer, and
//! the peers that matter — browsers — do not.

#![cfg(feature = "ws")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{Config, Target, TransportKind, bind};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as Frame;
use tokio_tungstenite::tungstenite::http::Request;

/// An OPTIONS whose `Via` names a WebSocket hop by an invented, unresolvable name — which is
/// what RFC 7118 §5.2 requires of a client that has no listening port, and therefore what sipx
/// will actually receive.
const OPTIONS: &str = "OPTIONS sip:sipx@example.com SIP/2.0\r\n\
     Via: SIP/2.0/WS df7jal23ls0d.invalid;branch=z9hG4bK776asdhds;rport\r\n\
     Max-Forwards: 70\r\n\
     To: <sip:sipx@example.com>\r\n\
     From: <sip:browser@example.com>;tag=b1\r\n\
     Call-ID: over-a-websocket@example.com\r\n\
     CSeq: 1 OPTIONS\r\n\
     Content-Length: 0\r\n\r\n";

/// An endpoint listening for WebSocket connections on an arbitrary port.
async fn sipx_listening() -> (
    sipx_transport::Handle,
    tokio::sync::mpsc::Receiver<sipx_transport::Incoming>,
    SocketAddr,
) {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.ws_server = Some(0);
    config.ws_keepalive = Duration::from_millis(50);
    let (handle, incoming) = bind(config).await.expect("binds");
    let addr = handle.ws_addr().expect("a WebSocket port was bound");
    (handle, incoming, addr)
}

/// A peer that speaks the `sip` subprotocol and nothing else about SIP.
async fn browser(addr: SocketAddr) -> WebSocketStream<TcpStream> {
    let stream = TcpStream::connect(addr).await.expect("connects");
    sipx_transport::ws::connect(stream, &addr.to_string(), "/", false)
        .await
        .expect("the upgrade is accepted")
}

/// W2, and the failing-first test this story names.
#[tokio::test]
async fn a_message_arrives_as_one_websocket_frame() {
    let (_handle, mut incoming, addr) = sipx_listening().await;
    let mut socket = browser(addr).await;

    socket
        .send(Frame::text(OPTIONS))
        .await
        .expect("the frame is sent");

    let request = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
        .await
        .expect("no timeout")
        .expect("a request");

    assert_eq!(request.transport, TransportKind::Ws);
    assert_eq!(request.request.method, Method::Options);
}

/// W4. The `Via` names `df7jal23ls0d.invalid`, which cannot be resolved and must never be
/// tried: the connection the request came in on is the only way back, and this is the whole
/// reason RFC 7118 §5.2 lets a client invent a name in the first place.
#[tokio::test]
async fn a_response_returns_over_the_same_connection() {
    let (handle, mut incoming, addr) = sipx_listening().await;
    let mut socket = browser(addr).await;

    socket.send(Frame::text(OPTIONS)).await.expect("sent");

    let request = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
        .await
        .expect("no timeout")
        .expect("a request");
    let response =
        ResponseBuilder::to_request(&request.request, StatusCode::new(200).expect("valid"), "OK")
            .expect("builds")
            .build();
    handle
        .respond(&request.key, response)
        .await
        .expect("responds");

    let frame = tokio::time::timeout(Duration::from_secs(5), next_sip(&mut socket))
        .await
        .expect("no timeout")
        .expect("a response on the same socket");
    assert!(
        frame.starts_with("SIP/2.0 200"),
        "expected the 200 back on this connection, got: {frame}"
    );
}

/// W1. Without the subprotocol there is no agreement about what the frames mean, so upgrading
/// anyway would leave sipx parsing whatever the peer happens to send as if it were SIP.
#[tokio::test]
async fn a_peer_that_does_not_offer_the_sip_subprotocol_is_refused() {
    let (_handle, _incoming, addr) = sipx_listening().await;
    let stream = TcpStream::connect(addr).await.expect("connects");

    // A complete, valid RFC 6455 upgrade — every header the handshake needs — differing from a
    // good one in exactly one respect: it names no subprotocol. Leaving anything else out would
    // have the *client* refuse its own request, and the test would pass without the server ever
    // having been asked.
    let request = Request::builder()
        .method("GET")
        .uri(format!("ws://{addr}/"))
        .header("Host", addr.to_string())
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .expect("a request");

    let error = tokio_tungstenite::client_async(request, stream)
        .await
        .expect_err("an upgrade that names no subprotocol must be refused");
    assert!(
        error.to_string().contains("400"),
        "the refusal must come from the server, not from our own request: {error}"
    );
}

/// And the mirror image: a *server* that upgrades without agreeing to the subprotocol has
/// agreed to nothing, and sipx must not take the connection on that basis.
#[tokio::test]
async fn a_server_that_ignores_the_subprotocol_is_refused() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let addr = listener.local_addr().expect("has an address");
    let accepting = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accepts");
        // A plain upgrade: valid RFC 6455, silent on RFC 7118.
        let _ = tokio_tungstenite::accept_async(stream).await;
    });

    let stream = TcpStream::connect(addr).await.expect("connects");
    let error = sipx_transport::ws::connect(stream, &addr.to_string(), "/", false)
        .await
        .expect_err("silence is not agreement");
    // Asserting the requirement, not the wording: what matters is that the connection is
    // refused and that the reason names the subprotocol.
    assert!(
        error.to_string().to_lowercase().contains("subprotocol"),
        "{error}"
    );

    accepting.abort();
}

/// W3. Half a message is not a message held over until the rest arrives — that is the *stream*
/// rule, and RFC 7118 §5 replaces it. The connection closes rather than trying to reassemble,
/// because a peer that frames wrongly has revealed it disagrees about where messages end.
#[tokio::test]
async fn a_message_split_across_two_frames_is_rejected() {
    let (_handle, mut incoming, addr) = sipx_listening().await;
    let mut socket = browser(addr).await;

    let (head, tail) = OPTIONS.split_at(OPTIONS.len() / 2);
    socket.send(Frame::text(head)).await.expect("sent");
    socket.send(Frame::text(tail)).await.expect("sent");

    // Nothing is delivered, and the connection is closed rather than left in a state where the
    // next frame would be parsed against a half-consumed buffer.
    let delivered = tokio::time::timeout(Duration::from_millis(400), incoming.recv()).await;
    assert!(
        delivered.is_err(),
        "a message split across frames must not be reassembled"
    );

    let closed = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = socket.next().await {
            if matches!(frame, Ok(Frame::Close(_)) | Err(_)) {
                return;
            }
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "the connection must be closed, not left open"
    );
}

/// Two messages in one frame is the same fault from the other side. Note what the alternative
/// would be: the datagram parser this borrows from ignores whatever follows a message, so a
/// pass-through implementation would deliver the first and silently drop the second.
#[tokio::test]
async fn two_messages_in_one_frame_are_rejected() {
    let (_handle, mut incoming, addr) = sipx_listening().await;
    let mut socket = browser(addr).await;

    socket
        .send(Frame::text(format!("{OPTIONS}{OPTIONS}")))
        .await
        .expect("sent");

    let delivered = tokio::time::timeout(Duration::from_millis(400), incoming.recv()).await;
    assert!(
        delivered.is_err(),
        "a frame carrying two messages carries neither"
    );
}

/// Intermediaries close sockets that have said nothing for a while. A registration whose
/// connection died silently is a phone that rings nowhere, so sipx keeps the path warm itself
/// rather than trusting the peer to.
#[tokio::test]
async fn an_idle_connection_is_pinged() {
    let (_handle, _incoming, addr) = sipx_listening().await;
    let mut socket = browser(addr).await;

    let pinged = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(frame)) = socket.next().await {
            if matches!(frame, Frame::Ping(_)) {
                return true;
            }
        }
        false
    })
    .await
    .expect("no timeout");

    assert!(pinged, "an idle websocket must be pinged");
}

/// sipx as the WebSocket *client*: the upgrade asks for `sip`, and the `Via` it writes is an
/// invented name rather than a socket address, because there is no port to connect back to.
#[tokio::test]
async fn sipx_dials_out_and_invents_a_via() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let addr = listener.local_addr().expect("has an address");

    let served = tokio::spawn(async move {
        let (stream, peer) = listener.accept().await.expect("accepts");
        let mut socket = sipx_transport::ws::accept(stream, peer)
            .await
            .expect("sipx asks for the sip subprotocol");
        let received = next_sip(&mut socket).await.expect("a request");

        // Answer, so the client's transaction completes rather than timing out.
        let request = sipx_sip::parse_datagram(
            Bytes::copy_from_slice(received.as_bytes()),
            &sipx_sip::Limits::stream(),
        )
        .expect("parses");
        let sipx_sip::Message::Request(request) = request else {
            panic!("expected a request");
        };
        let response =
            ResponseBuilder::to_request(&request, StatusCode::new(200).expect("valid"), "OK")
                .expect("builds")
                .build();
        socket
            .send(Frame::binary(
                sipx_sip::Message::Response(response).to_bytes(),
            ))
            .await
            .expect("responds");
        received
    });

    let (client, _rx) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let target = Target::new(addr, TransportKind::Ws);

    let mut responses = client
        .send(options_to("example.com"), target)
        .await
        .expect("sends");
    let response = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response over the websocket");
    assert_eq!(response.status.code(), 200);

    let sent = served.await.expect("the server finishes");
    let via = sent
        .lines()
        .find(|line| line.starts_with("Via:"))
        .expect("a Via");
    assert!(via.contains("SIP/2.0/WS "), "{via}");
    assert!(
        via.contains(".invalid"),
        "a WebSocket client has no address to advertise: {via}"
    );
}

/// W13. RFC 7118 §5 fixes no resource name, so a server is entitled to serve SIP anywhere it
/// likes — and one that serves it from its own HTTP server, at `/ws`, is not an exotic case but
/// the arrangement a second independent implementation ships with. A target that cannot say
/// which resource it wants can reach exactly one of those servers.
#[tokio::test]
async fn a_target_can_name_the_resource_the_peer_serves_sip_at() {
    let (addr, served) = a_peer_serving_sip_only_at("/ws").await;

    let (client, _rx) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let target = Target::new(addr, TransportKind::Ws).at_path("/ws");

    let mut responses = client
        .send(options_to("example.com"), target)
        .await
        .expect("sends");
    let response = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response from the resource the peer serves SIP at");
    assert_eq!(response.status.code(), 200);

    served
        .await
        .expect("the server finishes")
        .expect("the peer received the request");
}

/// And the reason the test above is not vacuous: this peer really does refuse the root. Without
/// this, a fixture that had quietly upgraded on any path would let the test pass whatever the
/// handshake asked for.
#[tokio::test]
async fn the_default_resource_is_refused_by_a_peer_that_serves_sip_elsewhere() {
    let (addr, _served) = a_peer_serving_sip_only_at("/ws").await;

    let (client, _rx) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    let mut responses = client
        .send(
            options_to("example.com"),
            Target::new(addr, TransportKind::Ws),
        )
        .await
        .expect("sends");
    let outcome = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("the refusal is immediate, not a transaction timeout");
    assert!(
        outcome.is_none(),
        "a 404 to the upgrade is a connection that never existed, not a SIP response: {outcome:?}"
    );
}

/// A WebSocket server that upgrades at one resource and answers `404 Not Found` anywhere else.
///
/// The handle yields the request the peer received, so a test asserting on the reply cannot pass
/// against a peer that was never reached.
// The refusal type is an HTTP response and is as large as one. Not ours to box: it is the shape
// the handshake callback must return, the same way `ws::accept`'s is.
#[allow(clippy::result_large_err)]
async fn a_peer_serving_sip_only_at(
    resource: &'static str,
) -> (SocketAddr, tokio::task::JoinHandle<Option<String>>) {
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
    use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode as HttpStatus};

    fn route(
        resource: &str,
        request: &Request,
        mut response: Response,
    ) -> Result<Response, ErrorResponse> {
        if request.uri().path() != resource {
            let mut refusal = ErrorResponse::new(Some(format!(
                "this server speaks SIP at {resource} and nowhere else"
            )));
            *refusal.status_mut() = HttpStatus::NOT_FOUND;
            return Err(refusal);
        }
        response.headers_mut().insert(
            "sec-websocket-protocol",
            HeaderValue::from_static(sipx_transport::ws::SUBPROTOCOL),
        );
        Ok(response)
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let addr = listener.local_addr().expect("has an address");

    let served = tokio::spawn(async move {
        // A loop rather than one `accept`: a refused upgrade is a connection that came and went,
        // and the peer stays up for the next one exactly as a real server would.
        loop {
            let (stream, _) = listener.accept().await.expect("accepts");
            let upgraded = tokio_tungstenite::accept_hdr_async(
                stream,
                |request: &Request, response: Response| route(resource, request, response),
            )
            .await;
            let Ok(mut socket) = upgraded else {
                continue;
            };
            let Some(received) = next_sip(&mut socket).await else {
                continue;
            };

            // Answer, so the client's transaction completes rather than timing out.
            let request = sipx_sip::parse_datagram(
                Bytes::copy_from_slice(received.as_bytes()),
                &sipx_sip::Limits::stream(),
            )
            .expect("parses");
            let sipx_sip::Message::Request(request) = request else {
                panic!("expected a request");
            };
            let response =
                ResponseBuilder::to_request(&request, StatusCode::new(200).expect("valid"), "OK")
                    .expect("builds")
                    .build();
            socket
                .send(Frame::binary(
                    sipx_sip::Message::Response(response).to_bytes(),
                ))
                .await
                .expect("responds");
            return Some(received);
        }
    });

    (addr, served)
}

/// The whole path, between two sipx endpoints — the one that dials has no listener of its own,
/// so everything must come back over the connection it opened.
#[tokio::test]
async fn two_endpoints_exchange_a_request_over_a_websocket() {
    let (server, mut server_rx, ws_addr) = sipx_listening().await;

    let responder = tokio::spawn(async move {
        let incoming = server_rx
            .recv()
            .await
            .expect("a request over the websocket");
        assert_eq!(incoming.transport, TransportKind::Ws);
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

    let (client, _rx) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let mut responses = client
        .send(
            options_to("example.com"),
            Target::new(ws_addr, TransportKind::Ws),
        )
        .await
        .expect("sends");

    let response = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response");
    assert_eq!(response.status.code(), 200);
    responder.await.expect("the responder finishes");
}

/// A WebSocket and a TCP connection to one address are two connections.
///
/// They can share a port — nothing stops a server offering both on 5060 — and handing SIP
/// framed one way to a peer expecting the other produces a connection that is open, silent and
/// wrong.
#[tokio::test]
async fn a_websocket_and_a_tcp_connection_to_one_address_are_not_interchangeable() {
    use sipx_transport::{ConnectionKey, Pool, PoolConfig};

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let local = listener.local_addr().expect("has an address");
    let accepting = tokio::spawn(async move { listener.accept().await.expect("accepts") });
    let _client = TcpStream::connect(local).await.expect("connects");

    let (events, _rx) = tokio::sync::mpsc::channel(64);
    let mut pool = Pool::new(
        PoolConfig::default(),
        sipx_sip::Limits::stream(),
        events.clone(),
    );

    let (stream, peer) = accepting.await.expect("accepted");
    pool.accept(stream, peer);

    assert!(pool.holds(&ConnectionKey::new(peer, TransportKind::Tcp)));
    assert!(
        !pool.holds(&ConnectionKey::new(peer, TransportKind::Ws)),
        "the same address over another transport is another connection"
    );
}

/// Read frames until one carries SIP, and return it as text.
async fn next_sip<S>(socket: &mut WebSocketStream<S>) -> Option<String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    while let Some(Ok(frame)) = socket.next().await {
        match frame {
            Frame::Text(text) => return Some(text.to_string()),
            Frame::Binary(data) => return Some(String::from_utf8_lossy(&data).into_owned()),
            _ => {}
        }
    }
    None
}

fn options_to(host: &'static str) -> sipx_sip::Request {
    RequestBuilder::new(
        Method::Options,
        Uri::sip(Host::Name(HostName::new(host).expect("valid"))),
    )
    .header(HeaderName::To, format!("<sip:callee@{host}>"))
    .expect("valid")
    .header(HeaderName::From, format!("<sip:caller@{host}>;tag=t1"))
    .expect("valid")
    .header(HeaderName::CallId, Bytes::from_static(b"ws-call@sipx"))
    .expect("valid")
    .cseq(1, &Method::Options)
    .expect("valid")
    .max_forwards(70)
    .build()
}
