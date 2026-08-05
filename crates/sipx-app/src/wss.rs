//! A general-purpose secure WebSocket client for non-SIP peers.
//!
//! **Experimental** (`A-8`): new with `A-20`, and nothing outside this crate constrains its
//! shape yet — the realtime bridge (`A-22`) is its intended first caller.
//!
//! An RFC 6455 client composed over the workspace's one TLS policy: the handshake is
//! `tokio-tungstenite`'s, the certificate verification is [`ClientTls`]'s, and there is no
//! second copy of either. The workspace builds its WebSocket dependency without TLS features
//! for exactly this reason — a second, subtly different certificate check is how one of the
//! two ends up weaker — so `wss` here is that dependency's handshake running over a stream
//! [`ClientTls`] already verified (RFC 8446), with the name checked being the host from the
//! URL the caller set out to reach, exactly as `docs/specs/sip-tls.md` §3 has it for SIP.
//!
//! This is deliberately **not** the SIP WebSocket client. `sipx-transport`'s client refuses
//! any peer that does not negotiate the `sip` subprotocol and puts exactly one SIP message in
//! each frame, by contract (RFC 7118 §4.2, §5), and stays that way. A non-SIP peer negotiates
//! whatever the caller names — or nothing, which is what most non-SIP services expect — and
//! its messages mean whatever the application says they mean.
//!
//! Three disciplines carry over from the rest of the workspace rather than being invented
//! here. **Cleartext is loopback-only**: `ws://` to anything but a loopback host is refused by
//! name, so an unencrypted path exists for fixtures and stand-in peers and for nothing else.
//! **Sizes are bounded before allocation**: the configured bound goes into the handshake, so
//! an oversize message is the decoder's typed refusal (RFC 6455 §5.2), not an allocation.
//! **Liveness is probed, and silence is typed**: the client answers the peer's Pings through
//! the protocol layer, sends its own on a cadence, and a peer silent past the grace surfaces
//! as [`WssError::Stalled`] rather than a hang — the session-binding discipline
//! (`docs/specs/session-binding.md`), client side.

use std::fmt;
use std::net::IpAddr;

use bytes::Bytes;
use futures_util::{SinkExt as _, StreamExt as _};
use sipx_transport::tls::{ClientTls, TlsError};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::error::CapacityError;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue, Uri};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as FrameError, Message};
use tokio_tungstenite::{WebSocketStream, client_async_with_config};

/// The default bound on one received message, frames included.
///
/// Generous for a control plane speaking JSON — the realtime events this exists for are a few
/// kilobytes of base64 audio each — and still small enough that a hostile peer buys nothing.
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 1 << 20;

/// RFC 6455 Ping cadence, matching the session binding's server side.
pub const DEFAULT_PING_INTERVAL: Duration = Duration::from_secs(30);

/// How long a Ping may remain unanswered before the peer is declared gone.
pub const DEFAULT_PING_GRACE: Duration = Duration::from_secs(10);

/// What can go wrong dialing or speaking to a non-SIP WebSocket peer.
///
/// TLS failures are **not** re-typed: a wrong-name or unknown-issuer certificate arrives as
/// the workspace's existing [`TlsError`], because the check that refused it is the same one
/// every other TLS client here verifies with.
#[derive(Debug)]
#[non_exhaustive]
pub enum WssError {
    /// The URL is not one this client can dial.
    Url {
        /// What the caller asked for.
        url: String,
        /// What was wrong with it.
        detail: String,
    },
    /// A request header the caller supplied is not a usable header.
    Header {
        /// The header's name as given.
        name: String,
        /// What was wrong with it.
        detail: String,
    },
    /// Cleartext `ws` to a host that is not loopback.
    ///
    /// Refused by the *name* the caller set out to reach, before any resolution or
    /// connection: a DNS answer must not decide whether encryption happens.
    Cleartext {
        /// The non-loopback host.
        host: String,
    },
    /// The TLS layer refused — the workspace's one policy, in its own words.
    Tls(TlsError),
    /// The TCP connection could not be opened.
    Connect {
        /// Who we were dialing.
        peer: String,
        /// What went wrong.
        detail: String,
    },
    /// The RFC 6455 upgrade itself failed.
    Handshake {
        /// Who we were talking to.
        peer: String,
        /// What went wrong.
        detail: String,
    },
    /// The peer upgraded without agreeing to the subprotocol the caller named.
    ///
    /// An upgrade that names nothing back is not an agreement (RFC 6455 §4.1), and taking the
    /// connection on that basis would be a guess about what the frames mean.
    Subprotocol {
        /// Who we were talking to.
        peer: String,
        /// What the caller asked for.
        offered: String,
    },
    /// The peer sent a message larger than the configured bound.
    ///
    /// Typed rather than allocated: the bound was installed in the decoder at the handshake,
    /// so the refusal happens where the peer's declared length is first observed
    /// (RFC 6455 §5.2).
    Oversize {
        /// Who sent it.
        peer: String,
        /// The declared size.
        size: usize,
        /// The configured bound it exceeded.
        limit: usize,
    },
    /// The peer answered nothing for a whole liveness grace.
    ///
    /// The typed close of the session-binding discipline: a Ping went out, the grace elapsed,
    /// and a path that cannot carry a Pong cannot carry anything the application sent either.
    Stalled {
        /// Who went silent.
        peer: String,
        /// The grace that elapsed unanswered.
        bound: Duration,
    },
    /// The transport failed mid-conversation.
    Transport {
        /// Who we were talking to.
        peer: String,
        /// What went wrong.
        detail: String,
    },
}

impl fmt::Display for WssError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url { url, detail } => {
                write!(
                    formatter,
                    "{url} is not a URL this client can dial: {detail}"
                )
            }
            Self::Header { name, detail } => {
                write!(formatter, "request header {name}: {detail}")
            }
            Self::Cleartext { host } => write!(
                formatter,
                "cleartext ws to {host} refused: an unencrypted WebSocket may reach loopback \
                 only; everything else travels over the workspace's TLS policy (RFC 8446)"
            ),
            Self::Tls(error) => error.fmt(formatter),
            Self::Connect { peer, detail } => {
                write!(formatter, "connecting to {peer}: {detail}")
            }
            Self::Handshake { peer, detail } => {
                write!(formatter, "websocket handshake with {peer}: {detail}")
            }
            Self::Subprotocol { peer, offered } => write!(
                formatter,
                "{peer} did not agree to the {offered} subprotocol (RFC 6455 §4.1)"
            ),
            Self::Oversize { peer, size, limit } => write!(
                formatter,
                "{peer} sent a message of {size} bytes against a bound of {limit} \
                 (RFC 6455 §5.2)"
            ),
            Self::Stalled { peer, bound } => write!(
                formatter,
                "{peer} answered nothing for {bound:?} after a liveness probe \
                 (RFC 6455 §5.5.2)"
            ),
            Self::Transport { peer, detail } => {
                write!(formatter, "websocket with {peer}: {detail}")
            }
        }
    }
}

impl std::error::Error for WssError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Tls(error) => Some(error),
            _ => None,
        }
    }
}

impl From<TlsError> for WssError {
    fn from(error: TlsError) -> Self {
        Self::Tls(error)
    }
}

/// The knobs a caller may set, each with the workspace's default.
#[derive(Debug, Clone)]
pub struct WssClientConfig {
    /// The bound on one received message, frames included, installed at the handshake.
    pub max_message_bytes: usize,
    /// How often to probe an idle peer with a Ping.
    pub ping_interval: Duration,
    /// How long a probe may go unanswered before the peer is declared gone.
    pub ping_grace: Duration,
}

impl Default for WssClientConfig {
    fn default() -> Self {
        Self {
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            ping_interval: DEFAULT_PING_INTERVAL,
            ping_grace: DEFAULT_PING_GRACE,
        }
    }
}

/// One upgrade the caller wants performed: a URL, its headers, and at most one subprotocol.
#[derive(Clone)]
pub struct WssRequest {
    url: String,
    headers: Vec<(HeaderName, HeaderValue)>,
    subprotocol: Option<String>,
}

impl fmt::Debug for WssRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Header names are shape; header values are secrets — `Authorization` carries a bearer
        // token, and a `Debug` that printed it would put it in whatever log the caller writes.
        formatter
            .debug_struct("WssRequest")
            .field("url", &self.url)
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("subprotocol", &self.subprotocol)
            .finish()
    }
}

impl WssRequest {
    /// An upgrade of `url` with no headers beyond the handshake's own and no subprotocol.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
            subprotocol: None,
        }
    }

    /// Add a request header — an `Authorization` bearer being the expected case.
    ///
    /// Typed refusal rather than a panic when the name or value is not a header, because the
    /// value may be configuration and configuration is network-adjacent input.
    pub fn header(mut self, name: &str, value: &str) -> Result<Self, WssError> {
        let refused = |detail: String| WssError::Header {
            name: name.to_owned(),
            detail,
        };
        let name = HeaderName::try_from(name).map_err(|error| refused(error.to_string()))?;
        let value = HeaderValue::try_from(value).map_err(|error| refused(error.to_string()))?;
        self.headers.push((name, value));
        Ok(self)
    }

    /// Offer exactly this subprotocol, and require the peer to agree to it.
    ///
    /// Without this call none is offered: RFC 6455 §1.9 makes subprotocols an application
    /// agreement, and inventing one a non-SIP peer never registered would refuse peers that
    /// would otherwise be fine.
    #[must_use]
    pub fn subprotocol(mut self, name: impl Into<String>) -> Self {
        self.subprotocol = Some(name.into());
        self
    }
}

/// The client: the workspace's TLS policy plus this crate's bounds, ready to dial.
#[derive(Debug, Clone)]
pub struct WssClient {
    tls: ClientTls,
    config: WssClientConfig,
}

impl WssClient {
    /// A client verifying with `tls`, under the default bounds.
    #[must_use]
    pub fn new(tls: ClientTls) -> Self {
        Self::with_config(tls, WssClientConfig::default())
    }

    /// A client verifying with `tls`, under the caller's bounds.
    #[must_use]
    pub fn with_config(tls: ClientTls, config: WssClientConfig) -> Self {
        Self { tls, config }
    }

    /// Dial, verify, and upgrade — one connection, or the first typed refusal on the way.
    pub async fn connect(&self, request: WssRequest) -> Result<WssConnection, WssError> {
        let target = Target::of(&request.url)?;
        if !target.secure && !target.loopback() {
            return Err(WssError::Cleartext {
                host: target.host.clone(),
            });
        }

        let stream = TcpStream::connect((target.host.as_str(), target.port))
            .await
            .map_err(|error| WssError::Connect {
                peer: target.authority.clone(),
                detail: error.to_string(),
            })?;

        let io: Box<dyn Io> = if target.secure {
            // The name checked is the host from the URL the caller set out to reach — never
            // anything resolution produced — for the reason `docs/specs/sip-tls.md` §3 gives:
            // a name DNS chose is a name whoever influences DNS chose.
            let name = sipx_transport::tls::verification_name(&target.host)?;
            let verified = self
                .tls
                .connector()
                .connect(name, stream)
                .await
                .map_err(|error| {
                    // Every verification failure arrives here with the reason attached, and it
                    // leaves as the workspace's existing typed error — the same shape the SIP
                    // transports report, because it is the same check refusing.
                    TlsError::Handshake {
                        peer: target.host.clone(),
                        detail: error.to_string(),
                    }
                })?;
            Box::new(verified)
        } else {
            Box::new(stream)
        };

        let upgrade = upgrade_request(&target, &request)?;
        let bounds = WebSocketConfig::default()
            .max_message_size(Some(self.config.max_message_bytes))
            .max_frame_size(Some(self.config.max_message_bytes));
        let (socket, response) = client_async_with_config(upgrade, io, Some(bounds))
            .await
            .map_err(|error| WssError::Handshake {
                peer: target.authority.clone(),
                detail: error.to_string(),
            })?;

        // The handshake already refuses a response that does not echo a subprotocol we asked
        // for; this stays because that is a *dependency's* behaviour, and a guarantee sipx
        // makes should not quietly become one sipx hopes someone else still makes.
        if let Some(offered) = &request.subprotocol
            && !agrees(response.headers(), offered)
        {
            return Err(WssError::Subprotocol {
                peer: target.authority.clone(),
                offered: offered.clone(),
            });
        }

        Ok(WssConnection {
            socket,
            peer: target.authority,
            probe: Instant::now() + self.config.ping_interval,
            awaiting_pong: false,
            ping_interval: self.config.ping_interval,
            ping_grace: self.config.ping_grace,
        })
    }
}

/// One message from the peer, in the application's terms rather than the frame layer's.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WssMessage {
    /// A text message (RFC 6455 §5.6 guarantees it is UTF-8).
    Text(String),
    /// A binary message.
    Binary(Bytes),
}

/// A live connection to a non-SIP peer.
///
/// Reading through [`next`](Self::next) is what keeps it alive: the peer's Pings are answered
/// there, this side's probes are sent there, and a peer silent past the grace is reported
/// there. A connection nobody reads is a connection nobody is checking on.
pub struct WssConnection {
    socket: WebSocketStream<Box<dyn Io>>,
    peer: String,
    /// When the liveness clock next fires: the next probe when idle, the deadline for the
    /// Pong once one is out.
    probe: Instant,
    awaiting_pong: bool,
    ping_interval: Duration,
    ping_grace: Duration,
}

impl fmt::Debug for WssConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WssConnection")
            .field("peer", &self.peer)
            .finish_non_exhaustive()
    }
}

impl WssConnection {
    /// Who the connection was verified against and upgraded with.
    #[must_use]
    pub fn peer(&self) -> &str {
        &self.peer
    }

    /// Send one text message.
    pub async fn send_text(&mut self, text: &str) -> Result<(), WssError> {
        let message = Message::Text(text.into());
        self.socket
            .send(message)
            .await
            .map_err(|error| self.fault(error))
    }

    /// Send one binary message.
    pub async fn send_binary(&mut self, bytes: Bytes) -> Result<(), WssError> {
        self.socket
            .send(Message::Binary(bytes))
            .await
            .map_err(|error| self.fault(error))
    }

    /// The next message, `Ok(None)` when the peer closed, or the typed failure.
    ///
    /// This is also where liveness runs. The clock only ever bounds a failure: the peer's own
    /// frames complete the wait whenever there are any, and the timer's sole verdicts are
    /// "probe now" and "the probe went unanswered".
    pub async fn next(&mut self) -> Result<Option<WssMessage>, WssError> {
        loop {
            tokio::select! {
                frame = self.socket.next() => match frame {
                    Some(Ok(Message::Text(text))) => {
                        return Ok(Some(WssMessage::Text(text.as_str().to_owned())));
                    }
                    Some(Ok(Message::Binary(data))) => {
                        return Ok(Some(WssMessage::Binary(data)));
                    }
                    Some(Ok(Message::Pong(_))) => {
                        // The peer is alive; stand down until the next cadence.
                        self.awaiting_pong = false;
                        self.probe = Instant::now() + self.ping_interval;
                    }
                    // Pings are answered by the protocol layer before this ever sees them
                    // (RFC 6455 §5.5.2); raw frames are the layer's own bookkeeping.
                    Some(Ok(Message::Ping(_) | Message::Frame(_))) => {}
                    Some(Ok(Message::Close(_))) | None => return Ok(None),
                    Some(Err(error)) => return Err(self.fault(error)),
                },
                // The liveness clock. In this race the clock is always the loser's branch — a
                // bound on failure, never a stand-in for the peer's next frame.
                () = tokio::time::sleep_until(self.probe) => {
                    if self.awaiting_pong {
                        return Err(WssError::Stalled {
                            peer: self.peer.clone(),
                            bound: self.ping_grace,
                        });
                    }
                    self.socket
                        .send(Message::Ping(Bytes::new()))
                        .await
                        .map_err(|error| self.fault(error))?;
                    self.awaiting_pong = true;
                    self.probe = Instant::now() + self.ping_grace;
                }
            }
        }
    }

    /// Close deliberately, so the peer learns this was not a network failure to retry through.
    pub async fn close(&mut self) -> Result<(), WssError> {
        match self.socket.close(None).await {
            Ok(()) | Err(FrameError::ConnectionClosed | FrameError::AlreadyClosed) => Ok(()),
            Err(error) => Err(self.fault(error)),
        }
    }

    /// The typed failure for one frame-layer error.
    fn fault(&self, error: FrameError) -> WssError {
        match error {
            FrameError::Capacity(CapacityError::MessageTooLong { size, max_size }) => {
                WssError::Oversize {
                    peer: self.peer.clone(),
                    size,
                    limit: max_size,
                }
            }
            other => WssError::Transport {
                peer: self.peer.clone(),
                detail: other.to_string(),
            },
        }
    }
}

/// The stream under the WebSocket: loopback cleartext or the TLS policy's verified stream.
///
/// A trait object rather than an enum, deliberately: naming the TLS stream's concrete type
/// would name the TLS library, and which one that is belongs to `sipx-transport`.
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> Io for S {}

/// Where one URL says to go, in the pieces the dial needs.
#[derive(Debug)]
struct Target {
    secure: bool,
    /// The host to verify against — brackets stripped, so an IPv6 literal parses.
    host: String,
    /// Host and port as the URL wrote them: the `Host` header, and the peer in every error.
    authority: String,
    port: u16,
    /// Path and query, `/` when the URL names none.
    resource: String,
}

impl Target {
    fn of(url: &str) -> Result<Self, WssError> {
        let refused = |detail: &str| WssError::Url {
            url: url.to_owned(),
            detail: detail.to_owned(),
        };
        let uri: Uri = url.parse().map_err(|_| refused("not a parseable URL"))?;
        let secure = match uri.scheme_str() {
            Some("wss") => true,
            Some("ws") => false,
            _ => return Err(refused("the scheme must be ws or wss")),
        };
        let authority = uri.authority().ok_or_else(|| refused("no host to dial"))?;
        let host = authority.host();
        if host.is_empty() {
            return Err(refused("no host to dial"));
        }
        // `Authority::host` keeps an IPv6 literal's brackets; the address inside them is what
        // connects, verifies, and answers the loopback question.
        let bare = host
            .strip_prefix('[')
            .and_then(|inner| inner.strip_suffix(']'))
            .unwrap_or(host);
        let port = authority
            .port_u16()
            .unwrap_or(if secure { 443 } else { 80 });
        let resource = uri
            .path_and_query()
            .map_or("/", |path| path.as_str())
            .to_owned();
        Ok(Self {
            secure,
            host: bare.to_owned(),
            authority: authority.as_str().to_owned(),
            port,
            resource,
        })
    }

    /// Whether the *name* is loopback — an address literal that says so, or `localhost`.
    ///
    /// A DNS name that would resolve to loopback does not count, deliberately: whether
    /// encryption happens must not be a question DNS gets to answer.
    fn loopback(&self) -> bool {
        self.host.eq_ignore_ascii_case("localhost")
            || self
                .host
                .parse::<IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    }
}

/// The upgrade request: RFC 6455 §4.1's required headers, the caller's own, and a subprotocol
/// only when one was named.
fn upgrade_request(
    target: &Target,
    request: &WssRequest,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, WssError> {
    let scheme = if target.secure { "wss" } else { "ws" };
    let mut builder = tokio_tungstenite::tungstenite::http::Request::builder()
        .method("GET")
        .uri(format!(
            "{scheme}://{}{}",
            target.authority, target.resource
        ))
        // The authority alone (RFC 9110 §7.2); the resource travels in the request-target.
        .header("Host", target.authority.clone())
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        );
    if let Some(subprotocol) = &request.subprotocol {
        builder = builder.header("Sec-WebSocket-Protocol", subprotocol.clone());
    }
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    builder.body(()).map_err(|error| WssError::Handshake {
        peer: target.authority.clone(),
        detail: error.to_string(),
    })
}

/// Whether these response headers name the agreed subprotocol (RFC 6455 §4.1 allows a list).
fn agrees(headers: &tokio_tungstenite::tungstenite::http::HeaderMap, offered: &str) -> bool {
    headers
        .get_all("sec-websocket-protocol")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case(offered))
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

    #[test]
    fn a_url_comes_apart_into_the_pieces_the_dial_needs() {
        let target = Target::of("wss://api.example.com/v1/realtime?model=x").expect("parses");
        assert!(target.secure);
        assert_eq!(target.host, "api.example.com");
        assert_eq!(target.authority, "api.example.com");
        assert_eq!(target.port, 443);
        assert_eq!(target.resource, "/v1/realtime?model=x");
    }

    #[test]
    fn an_explicit_port_travels_in_the_authority() {
        let target = Target::of("ws://127.0.0.1:8088/ws").expect("parses");
        assert!(!target.secure);
        assert_eq!(target.authority, "127.0.0.1:8088");
        assert_eq!(target.port, 8088);
    }

    #[test]
    fn a_bare_authority_still_has_a_resource() {
        let target = Target::of("wss://example.com").expect("parses");
        assert_eq!(target.resource, "/");
    }

    #[test]
    fn an_ipv6_literal_loses_its_brackets_for_verification_and_keeps_them_for_host() {
        let target = Target::of("ws://[::1]:9000/").expect("parses");
        assert_eq!(target.host, "::1");
        assert_eq!(target.authority, "[::1]:9000");
        assert!(target.loopback());
    }

    #[test]
    fn loopback_is_a_property_of_the_name_not_of_resolution() {
        for loopback in [
            "ws://localhost/",
            "ws://127.0.0.1/",
            "ws://127.8.9.1/",
            "ws://[::1]/",
        ] {
            assert!(
                Target::of(loopback).expect("parses").loopback(),
                "{loopback}"
            );
        }
        for elsewhere in [
            "ws://example.com/",
            "ws://192.0.2.1/",
            "ws://loopback.example/",
        ] {
            assert!(
                !Target::of(elsewhere).expect("parses").loopback(),
                "{elsewhere}"
            );
        }
    }

    #[test]
    fn a_scheme_that_is_not_websocket_is_refused_by_name() {
        let error = Target::of("https://example.com/").expect_err("refused");
        assert!(matches!(error, WssError::Url { .. }), "{error}");
    }

    #[test]
    fn the_upgrade_offers_a_subprotocol_only_when_one_is_named() {
        let target = Target::of("wss://example.com/x").expect("parses");
        let bare =
            upgrade_request(&target, &WssRequest::new("wss://example.com/x")).expect("a request");
        assert!(bare.headers().get("sec-websocket-protocol").is_none());

        let named = WssRequest::new("wss://example.com/x").subprotocol("chat.v1");
        let with = upgrade_request(&target, &named).expect("a request");
        assert_eq!(
            with.headers()
                .get("sec-websocket-protocol")
                .expect("offered"),
            "chat.v1"
        );
    }

    #[test]
    fn the_callers_headers_travel_on_the_upgrade() {
        let target = Target::of("wss://example.com/").expect("parses");
        let request = WssRequest::new("wss://example.com/")
            .header("Authorization", "Bearer token")
            .expect("a usable header");
        let upgrade = upgrade_request(&target, &request).expect("a request");
        assert_eq!(
            upgrade.headers().get("authorization").expect("travels"),
            "Bearer token"
        );
    }

    #[test]
    fn an_unusable_header_is_a_typed_refusal() {
        let error = WssRequest::new("wss://example.com/")
            .header("not a header name", "x")
            .expect_err("refused");
        assert!(matches!(error, WssError::Header { .. }), "{error}");
    }

    #[test]
    fn agreement_is_found_however_the_peer_lists_it() {
        let mut headers = tokio_tungstenite::tungstenite::http::HeaderMap::new();
        headers.append(
            "sec-websocket-protocol",
            HeaderValue::from_static("chat.v1"),
        );
        assert!(agrees(&headers, "chat.v1"));
        assert!(!agrees(&headers, "chat"));

        let mut listed = tokio_tungstenite::tungstenite::http::HeaderMap::new();
        listed.append(
            "sec-websocket-protocol",
            HeaderValue::from_static("audio, CHAT.V1"),
        );
        assert!(
            agrees(&listed, "chat.v1"),
            "a list, and case is not part of it"
        );

        assert!(!agrees(
            &tokio_tungstenite::tungstenite::http::HeaderMap::new(),
            "chat.v1"
        ));
    }
}
