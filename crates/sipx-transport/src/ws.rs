//! SIP over WebSocket (RFC 7118).
//!
//! Two things make this a transport of its own rather than TCP with a wrapper round it, and
//! both come from `docs/specs/sip-tls.md` §4.
//!
//! **The frame is the message.** RFC 7118 §5 puts exactly one SIP message in each WebSocket
//! message — not `Content-Length` framing, which is what every other stream transport here
//! uses. So a peer that sends half a message, or two, has not sent something sipx should try to
//! make sense of; it has revealed that it does not agree about where messages begin.
//!
//! **The client cannot be connected back to.** A browser has no listening port, so its `Via`
//! sent-by is an invented name that will never resolve, and everything sipx sends it goes back
//! over the connection it came in on. That is the RFC 5923 rule from the TCP transport made
//! absolute: here there is no fallback, because there is nowhere to fall back to.
//!
//! WSS is this module over the TLS from [`crate::tls`] — the same certificate policy and the
//! same code, because a second implementation of a security check is how one of the two ends up
//! weaker.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use sipx_sip::error::{FramingError, ParseError};
use sipx_sip::{Limits, Message, StreamParser, parse_datagram};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as Frame;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderMap, HeaderValue, StatusCode};
use tokio_tungstenite::{WebSocketStream, accept_hdr_async, client_async};

use crate::target::ConnectionKey;
use crate::tcp::Event;

/// The subprotocol name RFC 7118 §4.2 registers.
pub const SUBPROTOCOL: &str = "sip";

/// The header both halves of the handshake negotiate it in.
const PROTOCOL_HEADER: &str = "sec-websocket-protocol";

/// A negotiated WebSocket carrying SIP.
pub type Socket<S> = WebSocketStream<S>;

/// What can go wrong establishing a WebSocket.
#[derive(Debug, thiserror::Error)]
pub enum WsError {
    /// The peer never agreed to carry SIP.
    ///
    /// Refusing is the point rather than an inconvenience: without the subprotocol there is no
    /// agreement about what the frames mean, and guessing is how a stack ends up parsing
    /// somebody else's protocol as SIP.
    #[error("{peer} did not agree to the sip subprotocol (RFC 7118 §4.2)")]
    Subprotocol {
        /// Who we were talking to.
        peer: String,
    },
    /// The upgrade itself failed.
    #[error("websocket handshake with {peer}: {detail}")]
    Handshake {
        /// Who we were talking to.
        peer: String,
        /// What went wrong.
        detail: String,
    },
}

/// Perform the client half of the handshake, asking for the `sip` subprotocol at `path`.
///
/// `authority` is the host and port from the URI — what goes in `Host`, and under WSS the name
/// the certificate had to be valid for. `path` is the resource to ask for: RFC 7118 §5 does not
/// fix one, so where a server serves SIP is the server's business and the caller's to know.
pub async fn connect<S>(
    stream: S,
    authority: &str,
    path: &str,
    secure: bool,
) -> Result<Socket<S>, WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let failed = |detail: String| WsError::Handshake {
        peer: authority.to_owned(),
        detail,
    };

    let request =
        upgrade_request(authority, path, secure).map_err(|error| failed(error.to_string()))?;

    let (socket, response) = client_async(request, stream)
        .await
        .map_err(|error| failed(error.to_string()))?;

    // A server that ignores the subprotocol and upgrades anyway has agreed to nothing, and
    // taking the connection on that basis is exactly the guess RFC 7118 §4.2 forbids.
    //
    // Redundant today: the handshake below already refuses a response that does not echo a
    // subprotocol we asked for, so this never fires. It stays because that is a *dependency's*
    // behaviour, and a guarantee sipx makes should not quietly become a guarantee sipx hopes
    // someone else still makes.
    if !offers_sip(response.headers()) {
        return Err(WsError::Subprotocol {
            peer: authority.to_owned(),
        });
    }

    Ok(socket)
}

/// The upgrade request sipx sends.
///
/// Written out in full rather than half-specified: the handshake takes what it is given and
/// refuses a request missing any of these, so there is nothing gained by leaving one out and a
/// confusing failure to be had by trying.
///
/// `Sec-WebSocket-Key` is a fresh nonce whose echo proves the peer understood the upgrade rather
/// than being an HTTP server that says yes to everything (RFC 6455 §4.1).
///
/// The URI carries the resource and `Host` does not. They come apart precisely here: `Host` is
/// the authority alone (RFC 7230 §5.4), while the request-target is the path the server matches
/// its routes against — and a server serving SIP at `/ws` answers `404` to the `/` this used to
/// send unconditionally.
fn upgrade_request(
    authority: &str,
    path: &str,
    secure: bool,
) -> Result<
    tokio_tungstenite::tungstenite::http::Request<()>,
    tokio_tungstenite::tungstenite::http::Error,
> {
    let scheme = if secure { "wss" } else { "ws" };
    tokio_tungstenite::tungstenite::http::Request::builder()
        .method("GET")
        .uri(format!("{scheme}://{authority}{path}"))
        .header("Host", authority)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .header(PROTOCOL_HEADER, SUBPROTOCOL)
        .body(())
}

/// Perform the server half, refusing a peer that does not offer the `sip` subprotocol.
// The refusal type is an HTTP response and is as large as one. It is not ours to box: it is the
// shape the handshake callback must return.
#[allow(clippy::result_large_err)]
pub async fn accept<S>(stream: S, peer: SocketAddr) -> Result<Socket<S>, WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    accept_hdr_async(stream, |request: &Request, mut response: Response| {
        if !offers_sip(request.headers()) {
            let mut refusal = ErrorResponse::new(Some(format!(
                "this endpoint speaks the {SUBPROTOCOL} subprotocol only (RFC 7118 §4.2)"
            )));
            *refusal.status_mut() = StatusCode::BAD_REQUEST;
            return Err(refusal);
        }
        // Echoing it is what makes the negotiation two-sided: the client is entitled to check
        // this answer just as sipx checks the server's.
        response
            .headers_mut()
            .insert(PROTOCOL_HEADER, HeaderValue::from_static(SUBPROTOCOL));
        Ok(response)
    })
    .await
    .map_err(|error| WsError::Handshake {
        peer: peer.to_string(),
        detail: error.to_string(),
    })
}

/// A `Via` sent-by for an endpoint that can never be connected back to (RFC 7118 §5.2).
///
/// `.invalid` is reserved by RFC 2606 and guaranteed never to resolve, which is the whole
/// point: nothing anywhere must ever try to open a connection to it. Advertising a real
/// address here would be worse than useless — it would send a proxy off to a port that is not
/// listening while the connection it should have used sits open.
#[must_use]
pub fn invented_sent_by() -> String {
    use rand::Rng;
    let value: u64 = rand::rng().random();
    format!("{value:016x}.invalid")
}

/// Whether these headers name the `sip` subprotocol.
///
/// A peer may offer several, comma-separated or in repeated headers; RFC 6455 §4.1 allows both.
fn offers_sip(headers: &HeaderMap) -> bool {
    headers
        .get_all(PROTOCOL_HEADER)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case(SUBPROTOCOL))
}

/// Handshake as a client and then pump, reporting a failure the same way a refused connection
/// is reported — because to everything upstream that is what it is.
pub(crate) async fn dial<S>(
    stream: S,
    authority: &str,
    key: ConnectionKey,
    outgoing: mpsc::Receiver<Bytes>,
    events: mpsc::Sender<Event>,
    limits: Limits,
    keepalive: Duration,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let secure = key.transport == crate::target::TransportKind::Wss;
    // The resource travels on the key rather than beside it, because it is part of what makes
    // this connection this connection — see [`ConnectionKey`].
    match connect(stream, authority, key.ws_path(), secure).await {
        Ok(socket) => pump(socket, key, outgoing, events, limits, keepalive).await,
        Err(error) => {
            tracing::warn!(%error, peer = %key.peer, "websocket handshake failed");
            let _ = events.send(Event::Closed { key }).await;
        }
    }
}

/// Read and write one WebSocket until it ends.
pub(crate) async fn pump<S>(
    socket: Socket<S>,
    key: ConnectionKey,
    mut outgoing: mpsc::Receiver<Bytes>,
    events: mpsc::Sender<Event>,
    limits: Limits,
    keepalive: Duration,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (peer, transport) = (key.peer, key.transport);
    let (mut sink, mut source) = socket.split();

    // Intermediaries close sockets that have said nothing for a while, and a registration whose
    // connection has silently died is a phone that rings nowhere. A Ping (RFC 6455 §5.5.2) is
    // the cheapest thing that keeps the path open and, because the peer must answer it, the
    // only one that also tells us the path is still there.
    let mut ping = tokio::time::interval(keepalive);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await;

    loop {
        tokio::select! {
            frame = source.next() => {
                let payload = match frame {
                    Some(Ok(Frame::Text(text))) => Bytes::from(text),
                    Some(Ok(Frame::Binary(data))) => data,
                    // Pongs are answered by the protocol layer before this ever sees them.
                    Some(Ok(Frame::Ping(_) | Frame::Pong(_) | Frame::Frame(_))) => continue,
                    Some(Ok(Frame::Close(_))) | None => break,
                    Some(Err(error)) => {
                        tracing::debug!(%error, %peer, "websocket read failed");
                        break;
                    }
                };
                match parse_one(payload, &limits) {
                    Ok(message) => {
                        if events
                            .send(Event::Message {
                                message: Box::new(message),
                                source: peer,
                                transport,
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(detail) => {
                        // The same reasoning as a `Content-Length` framing error on TCP: once
                        // the two ends disagree about where a message ends, nothing further
                        // from this peer can be trusted to be what it claims.
                        tracing::debug!(%peer, %detail, "closing on a malformed websocket message");
                        break;
                    }
                }
            },
            Some(bytes) = outgoing.recv() => {
                if sink.send(frame_for(bytes)).await.is_err() {
                    break;
                }
            }
            _ = ping.tick() => {
                if sink.send(Frame::Ping(Bytes::new())).await.is_err() {
                    break;
                }
            }
        }
    }

    // A close frame, so the peer learns this was deliberate rather than a network failure it
    // should retry through.
    let _ = sink.close().await;
    let _ = events.send(Event::Closed { key }).await;
}

/// Parse exactly one SIP message out of one WebSocket message.
///
/// Strict, and deliberately stricter than the datagram parser it borrows from. RFC 3261 §18.3
/// says octets after a message in a *datagram* are noise to be ignored; RFC 7118 §5 says a
/// WebSocket message carries one SIP message and no more, so the same octets here mean the peer
/// is framing wrongly.
fn parse_one(frame: Bytes, limits: &Limits) -> Result<Message, String> {
    // The stream parser is reused rather than reimplemented: it already knows every rule about
    // where a message ends, and a second copy of those rules is a second place for them to
    // drift. What differs is only what is done with the answer — here, anything other than
    // exactly one whole message is a fault.
    let mut parser = StreamParser::new(*limits);
    match parser.push(&frame) {
        Ok(mut messages) => {
            let trailing = parser.pending();
            if trailing == 0
                && messages.len() == 1
                && let Some(message) = messages.pop()
            {
                return Ok(message);
            }
            Err(format!(
                "a WebSocket message carries exactly one SIP message (RFC 7118 §5); \
                 this one held {} complete and {trailing} octets of another",
                messages.len()
            ))
        }
        // The one framing rule that does not carry over. `Content-Length` is mandatory on a
        // stream because nothing else says where a message ends. Here the frame says, so a
        // message without one is legal and its body runs to the end of the frame — which is
        // what RFC 3261 §20.14 already prescribes wherever the transport delimits.
        Err(ParseError::Framing(FramingError::ContentLengthRequired)) => {
            parse_datagram(frame, limits).map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Put a message in a frame.
///
/// Text where the bytes allow it. RFC 7118 §5 permits either, and text is what a browser's
/// network panel and every WebSocket capture tool will show as readable SIP; a body that is not
/// valid UTF-8 leaves binary as the only correct choice.
fn frame_for(bytes: Bytes) -> Frame {
    match tokio_tungstenite::tungstenite::Utf8Bytes::try_from(bytes.clone()) {
        Ok(text) => Frame::Text(text),
        Err(_) => Frame::Binary(bytes),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    const OPTIONS: &str = "OPTIONS sip:a@b.com SIP/2.0\r\n\
         Via: SIP/2.0/WS df7jal23ls0d.invalid;branch=z9hG4bKx\r\n\
         To: <sip:a@b.com>\r\n\
         From: <sip:c@d.net>;tag=1\r\n\
         Call-ID: x@y\r\n\
         CSeq: 1 OPTIONS\r\n\
         Content-Length: 0\r\n\r\n";

    fn parse(text: &str) -> Result<Message, String> {
        parse_one(Bytes::copy_from_slice(text.as_bytes()), &Limits::stream())
    }

    #[test]
    fn one_message_in_one_frame_is_parsed() {
        parse(OPTIONS).expect("one message in one frame");
    }

    /// W3, and the half of RFC 7118 §5 that is easy to get wrong: this is precisely what the
    /// TCP transport is *supposed* to accept, held over until the rest arrives.
    #[test]
    fn a_message_split_across_frames_is_malformed() {
        let half = &OPTIONS[..OPTIONS.len() / 2];
        let error = parse(half).expect_err("half a message is not a message");
        assert!(error.contains("exactly one"), "{error}");
    }

    /// The other half: two messages in one frame is not two messages, it is a framing fault.
    /// Note that the *datagram* parser would quietly return the first and drop the second.
    #[test]
    fn two_messages_in_one_frame_are_malformed() {
        let error = parse(&format!("{OPTIONS}{OPTIONS}")).expect_err("two is not one");
        assert!(error.contains("exactly one"), "{error}");
    }

    /// Trailing octets are the same fault, and the case that separates this from
    /// `parse_datagram` — there they are noise the RFC says to ignore.
    #[test]
    fn octets_after_the_message_are_malformed() {
        parse(&format!("{OPTIONS}garbage"))
            .expect_err("a frame holds one message and nothing else");
    }

    /// A frame delimits, so `Content-Length` is not load-bearing the way it is on a stream.
    /// Refusing here would reject messages the transport can frame perfectly well.
    #[test]
    fn a_message_without_content_length_is_accepted() {
        let without = OPTIONS.replace("Content-Length: 0\r\n", "");
        parse(&without).expect("the frame says where it ends");
    }

    #[test]
    fn a_frame_holding_nothing_like_sip_is_refused() {
        parse("hello").expect_err("not a SIP message");
    }

    #[test]
    fn a_sip_message_travels_as_text() {
        assert!(matches!(
            frame_for(Bytes::copy_from_slice(OPTIONS.as_bytes())),
            Frame::Text(_)
        ));
    }

    /// A body that is not UTF-8 cannot go in a text frame — RFC 6455 §5.6 requires text frames
    /// to be valid UTF-8, and a peer must fail the connection when they are not.
    #[test]
    fn a_binary_body_travels_as_binary() {
        assert!(matches!(
            frame_for(Bytes::from_static(b"MESSAGE sip:a SIP/2.0\r\n\r\n\xff\xfe")),
            Frame::Binary(_)
        ));
    }

    #[test]
    fn the_subprotocol_is_found_however_it_is_offered() {
        let mut headers = HeaderMap::new();
        headers.append(PROTOCOL_HEADER, HeaderValue::from_static("sip"));
        assert!(offers_sip(&headers));

        let mut listed = HeaderMap::new();
        listed.append(PROTOCOL_HEADER, HeaderValue::from_static("chat, SIP, echo"));
        assert!(
            offers_sip(&listed),
            "comma-separated, and case is not part of it"
        );

        let mut repeated = HeaderMap::new();
        repeated.append(PROTOCOL_HEADER, HeaderValue::from_static("chat"));
        repeated.append(PROTOCOL_HEADER, HeaderValue::from_static("sip"));
        assert!(offers_sip(&repeated), "repeated headers are one list");

        let mut other = HeaderMap::new();
        other.append(PROTOCOL_HEADER, HeaderValue::from_static("chat, sipx"));
        assert!(!offers_sip(&other), "a longer token is a different token");

        assert!(
            !offers_sip(&HeaderMap::new()),
            "offering none is not offering sip"
        );
    }

    /// The line this transport used to get wrong. The request-target was `/` whatever the
    /// caller wanted, so a server serving SIP anywhere else answered `404` and the connection
    /// simply never existed — see `docs/specs/sip-tls.md` §6, W13.
    #[test]
    fn the_upgrade_asks_for_the_resource_it_was_given() {
        let request = upgrade_request("127.0.0.1:8088", "/ws", false).expect("a request");
        assert_eq!(request.uri().to_string(), "ws://127.0.0.1:8088/ws");
        assert_eq!(request.uri().path(), "/ws");
        // `Host` is the authority and nothing else (RFC 7230 §5.4). The resource belongs in the
        // request-target, which is where a server looks for it.
        assert_eq!(
            request.headers().get("Host").expect("a Host"),
            "127.0.0.1:8088"
        );
    }

    #[test]
    fn the_root_is_still_what_a_caller_naming_nothing_asks_for() {
        let request = upgrade_request("127.0.0.1:5060", "/", false).expect("a request");
        assert_eq!(request.uri().to_string(), "ws://127.0.0.1:5060/");
    }

    /// WSS changes the scheme and nothing else about where the resource goes.
    #[test]
    fn a_secure_upgrade_keeps_the_resource() {
        let request = upgrade_request("sipx.test:443", "/ws", true).expect("a request");
        assert_eq!(request.uri().to_string(), "wss://sipx.test:443/ws");
    }

    /// Servers do exist that want a token in the query, and RFC 7118 §5 no more forbids that
    /// than it fixes the path. Whatever the caller named is what is asked for.
    #[test]
    fn a_query_string_survives_the_handshake() {
        let request = upgrade_request("127.0.0.1:8088", "/ws?token=abc", false).expect("a request");
        assert_eq!(request.uri().path(), "/ws");
        assert_eq!(request.uri().query(), Some("token=abc"));
    }

    /// It must never resolve, and it must never be the same twice: RFC 7118 §5.2 asks for a
    /// unique name so two clients behind one proxy stay distinguishable.
    #[test]
    fn an_invented_sent_by_is_unresolvable_and_unique() {
        let one = invented_sent_by();
        assert!(one.ends_with(".invalid"), "{one}");
        assert_ne!(one, invented_sent_by());
    }
}
