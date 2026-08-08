//! A general-purpose secure WebSocket client for non-SIP peers.
//!
//! **Supported** (`A-22`): the shipped realtime host path constrains this client's authentication,
//! liveness, size, close and certificate semantics end to end.
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
//!
//! That liveness rule is **strict about Pongs, deliberately**, and a caller should know what it
//! buys. Only a Pong stands a probe down; data never does. A peer streaming at full rate while
//! withholding Pongs is therefore declared [`Stalled`](WssError::Stalled) mid-burst, at the
//! grace, once this side has taken delivery of what had already arrived — the same reading
//! `docs/specs/session-binding.md` states and `sipx-transport`'s server side already applies. A
//! path that has stopped carrying control frames has stopped being a path this side can steer,
//! whatever is still arriving on it, and `A-22` inherits that knowingly rather than by accident.
//!
//! The one thing that *does* survive the grace is an answer that beat it: a Pong which arrived
//! while the caller was away from [`next`](WssConnection::next) counts, because the peer sent it
//! in time and only this side was late to look. Nothing the peer sends after the grace expired
//! can extend it.
//!
//! **Credentials never travel in the URL.** A `wss://user:secret@host/` URL is refused
//! ([`WssError::Url`]): RFC 3986 §3.2.1 deprecates userinfo for exactly this reason, no `Host`
//! header may carry it (RFC 9110 §7.2), and a URL is the one field an application logs without
//! thinking. Authenticate with [`WssRequest::header`], whose values this module keeps out of
//! `Debug` on purpose.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt;
use std::net::IpAddr;

use bytes::Bytes;
use futures_util::{FutureExt as _, SinkExt as _, StreamExt as _};
use sipx_transport::tls::{ClientTls, TlsError};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::error::{CapacityError, ProtocolError};
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

/// How many already-buffered frames the liveness search takes before offering the executor a
/// turn. Nothing here waits on a clock; this only keeps a talkative peer from monopolising the
/// task while its backlog is read.
const YIELD_EVERY: usize = 64;

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
        /// What the caller asked for, with any RFC 3986 userinfo replaced by `***`.
        ///
        /// Redacted rather than echoed: one of the ways a URL is unusable here is that it
        /// carries credentials, and an error naming them would put them in the caller's log —
        /// which is the leak the refusal exists to prevent.
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
        /// The HTTP status the peer answered the upgrade with, when it answered with one.
        ///
        /// A peer that refuses the upgrade does so with a status before the 101 (RFC 6455 §4.1),
        /// and that is a different fact from "the upgrade broke": a caller authenticating with a
        /// bearer has to tell a rejected credential from an unreachable host, and reading the
        /// status back out of [`detail`](Self::Handshake::detail) would be parsing a `Display`
        /// string. `None` when the upgrade failed before any response — a malformed request, a
        /// truncated stream — because there was no status to have.
        status: Option<u16>,
        /// What went wrong.
        detail: String,
    },
    /// The peer did not agree to the subprotocol the caller named.
    ///
    /// An upgrade that names nothing back is not an agreement (RFC 6455 §4.1), and taking the
    /// connection on that basis would be a guess about what the frames mean. The check itself
    /// is the handshake's — it refuses the response before this module sees it — and this
    /// variant is that refusal given the workspace's own name, so a caller can tell "we do not
    /// speak the same protocol" from "the upgrade broke" without reading an error string.
    Subprotocol {
        /// Who we were talking to.
        peer: String,
        /// What the caller asked for — empty when the caller named none and the peer answered
        /// with one anyway, which RFC 6455 §4.1 fails the connection for just the same.
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
            Self::Handshake {
                peer,
                status,
                detail,
            } => match status {
                Some(status) => write!(
                    formatter,
                    "websocket handshake with {peer} was refused {status}: {detail}"
                ),
                None => write!(formatter, "websocket handshake with {peer}: {detail}"),
            },
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
        // The URL is the same hazard by another route: `connect` refuses userinfo, but a caller
        // may print the request without ever dialing, so the redaction has to live here too.
        formatter
            .debug_struct("WssRequest")
            .field("url", &redacted(&self.url))
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
        let (socket, _response) = client_async_with_config(upgrade, io, Some(bounds))
            .await
            .map_err(|error| {
                // RFC 6455 §4.1 step 6 is the handshake's own: it refuses a response that
                // echoes no subprotocol, or one nobody offered, before this module is handed
                // anything. Re-typed rather than re-checked, because a second check here could
                // only ever disagree with the one that already ran — and a disagreement in
                // favour of *accepting* would be this module deciding what frames mean.
                match error {
                    // All three of the dependency's subprotocol refusals, including the peer
                    // that names one nobody offered — RFC 6455 §4.1 step 6 fails the connection
                    // for that too, and it is the same disagreement seen from the other side.
                    FrameError::Protocol(ProtocolError::SecWebSocketSubProtocolError(_)) => {
                        WssError::Subprotocol {
                            peer: target.authority.clone(),
                            offered: request.subprotocol.clone().unwrap_or_default(),
                        }
                    }
                    // A response that is not a 101 is the peer's answer, not a broken pipe, and
                    // the status is the only part of it a caller can act on — `A-22` reads a
                    // 4xx here as a refused credential (`docs/specs/openai-realtime.md` §6).
                    FrameError::Http(response) => WssError::Handshake {
                        peer: target.authority.clone(),
                        status: Some(response.status().as_u16()),
                        detail: format!("the peer answered {}", response.status()),
                    },
                    other => WssError::Handshake {
                        peer: target.authority.clone(),
                        status: None,
                        detail: other.to_string(),
                    },
                }
            })?;

        Ok(WssConnection {
            socket,
            peer: target.authority,
            probe: Instant::now() + self.config.ping_interval,
            awaiting_pong: false,
            ping_interval: self.config.ping_interval,
            ping_grace: self.config.ping_grace,
            held: VecDeque::new(),
            closed: false,
            closed_with: None,
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
    /// Messages taken from the socket while looking for a Pong, handed to the caller in the order
    /// the peer sent them. Bounded by what one liveness pass found already buffered.
    ///
    /// **They survive every ending but one.** A close read ahead of them is reported after they
    /// are drained, so a peer that says goodbye mid-burst costs nothing; a
    /// [`Stalled`](WssError::Stalled) discards them, and that is deliberate rather than an
    /// oversight the sentence above used to deny (`A-22`'s review of `A-20`). The grace expired
    /// with no Pong, so this side has just declared the path unsteerable — handing over more of
    /// what arrived on it would be reporting progress on a connection the caller has been told to
    /// abandon. The queue also has no aggregate cap: it holds one liveness pass's worth of
    /// already-buffered frames, which for a burst source is exactly the burst.
    held: VecDeque<WssMessage>,
    /// The peer closed while liveness was reading ahead; reported once `held` is empty.
    closed: bool,
    /// The RFC 6455 §5.5.1 status code the peer's close frame carried, if it sent one.
    closed_with: Option<u16>,
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

    /// The status code the peer's close frame carried, once [`next`](Self::next) has reported the
    /// close by returning `Ok(None)`.
    ///
    /// `None` before that, and `None` afterwards when the connection ended without a code — an
    /// EOF, an abrupt reset, or a close frame with an empty body, none of which carry one
    /// (RFC 6455 §5.5.1, §7.1.5). Kept here rather than returned from `next` because a close is
    /// not a message: a caller reading messages should not have to destructure one to get the
    /// next event, and the code is a fact about the *connection* that outlives the read.
    #[must_use]
    pub fn close_code(&self) -> Option<u16> {
        self.closed_with
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
    ///
    /// **Cancel-safe.** Every piece of liveness state lives on the connection rather than in
    /// the future, so a caller that drops this — a `select!` arm that lost, a `timeout` that
    /// fired — loses no message and no probe. One cancellation is visible, and only as extra
    /// work: dropped inside the probe's own send, the Ping may reach the peer with this side's
    /// bookkeeping unwritten, which costs one redundant Ping at the next cadence and can never
    /// produce a [`Stalled`](WssError::Stalled) that was not owed.
    pub async fn next(&mut self) -> Result<Option<WssMessage>, WssError> {
        loop {
            // Anything liveness set aside while looking for a Pong is the caller's, and it is
            // older than whatever the socket holds now.
            if let Some(message) = self.held.pop_front() {
                return Ok(Some(message));
            }
            if self.closed {
                return Ok(None);
            }
            tokio::select! {
                // `biased`, with the clock read first, and it changes only one thing: when the
                // deadline has *already* passed, the work it owes happens before another of the
                // peer's frames is handed over. An unbiased choice here would let a peer that
                // keeps the socket readable defer the verdict frame by frame for as long as it
                // kept talking — the burst outrunning the bound `session-binding.md` sets.
                // Until the deadline passes this arm is simply pending, so it never delays a
                // frame and never stands in for one: the clock still only bounds a failure.
                biased;

                () = tokio::time::sleep_until(self.probe) => {
                    if self.awaiting_pong {
                        if self.answered_while_away().await? {
                            self.awaiting_pong = false;
                            self.probe = Instant::now() + self.ping_interval;
                            continue;
                        }
                        if self.closed {
                            // A peer that closed is not a peer that went silent; the loop head
                            // hands over what arrived before the close, then reports it.
                            continue;
                        }
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
                    Some(Ok(Message::Close(frame))) => {
                        self.closed_with = frame.map(|frame| u16::from(frame.code));
                        return Ok(None);
                    }
                    None => return Ok(None),
                    Some(Err(error)) => return Err(self.fault(error)),
                },
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

    /// Whether a Pong was **already** in hand when the grace expired.
    ///
    /// A grace that expires means only that this side has not *read* a Pong, and between calls
    /// to [`next`](Self::next) nobody is reading: a Pong that arrived while the caller was away
    /// is sitting in the socket, and on re-entry both `select!` branches are ready at once — an
    /// unbiased choice would announce [`Stalled`](WssError::Stalled) with the answer in hand.
    /// `sipx-transport`'s server side needs no equivalent because its loop is always parked on
    /// the socket; a caller-driven `next` is away exactly as often as its caller is busy.
    ///
    /// So this takes delivery of everything the socket holds *right now*, looking for a Pong.
    /// Data frames are set aside for the caller rather than returned from here, because a data
    /// frame is not an answer: were the search to stop at one, a peer streaming at full rate
    /// while withholding Pongs would defer the verdict for as long as it kept talking, and the
    /// discipline `docs/specs/session-binding.md` states — no Pong within the grace and the
    /// session is dead — would be unreachable while the connection was busiest.
    ///
    /// The work is bounded by what has already arrived: the search ends the moment the socket
    /// has nothing ready, so nothing the peer sends *after* the grace expired can extend it.
    async fn answered_while_away(&mut self) -> Result<bool, WssError> {
        let mut taken: usize = 0;
        loop {
            // Polls once and takes only what is ready. Nothing here waits, so a peer that has
            // genuinely stopped still reaches its verdict on this pass rather than the next.
            let Some(frame) = self.socket.next().now_or_never() else {
                return Ok(false);
            };
            match frame {
                Some(Ok(Message::Pong(_))) => return Ok(true),
                Some(Ok(Message::Text(text))) => {
                    self.held
                        .push_back(WssMessage::Text(text.as_str().to_owned()));
                }
                Some(Ok(Message::Binary(data))) => {
                    self.held.push_back(WssMessage::Binary(data));
                }
                Some(Ok(Message::Ping(_) | Message::Frame(_))) => {}
                Some(Ok(Message::Close(frame))) => {
                    // Reported after the caller has had what came before it.
                    self.closed_with = frame.map(|frame| u16::from(frame.code));
                    self.closed = true;
                    return Ok(false);
                }
                None => {
                    self.closed = true;
                    return Ok(false);
                }
                Some(Err(error)) => return Err(self.fault(error)),
            }
            taken += 1;
            if taken.is_multiple_of(YIELD_EVERY) {
                // A peer can keep this socket readable — a flood of control frames adds nothing
                // to `held` and would otherwise spin here without ever reaching the executor.
                // Yielding is not a wait: it costs this task its turn, never a wall-clock hold.
                tokio::task::yield_now().await;
            }
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

/// One URL with any RFC 3986 §3.2.1 userinfo replaced by `***`, safe to print.
///
/// Read off the raw string rather than a parsed URL on purpose: a URL too malformed to parse
/// still carries whatever secret was written into it, and that is exactly the URL an error
/// message wants to quote back.
///
/// The scheme is optional here, and that is the point. `user:secret@host` and `//user:secret@host`
/// both parse as a [`Uri`] — without a scheme, so both are refused a few lines below and both
/// reach an error with the credential still in them. A pasted URL that lost its `wss://` is the
/// ordinary way to write one.
///
/// What this does **not** cover is a credential in the query string, which is left as written:
/// `docs/specs/openai-realtime.md` §2 puts authentication in a header, so a token in a query
/// would be a caller doing something this client does not ask for, and blanket-redacting query
/// values would hide the `?model=` an operator needs to read.
fn redacted(url: &str) -> Cow<'_, str> {
    let start = match url.find("://") {
        Some(mark) => mark + "://".len(),
        // No scheme: an authority may still open the string, with or without `//`.
        None if url.starts_with("//") => "//".len(),
        None => 0,
    };
    // The authority ends where the path, query or fragment begins (RFC 3986 §3.2); anything
    // after that is not credentials however many `@` it contains.
    let end = url[start..]
        .find(['/', '?', '#'])
        .map_or(url.len(), |offset| start + offset);
    // The *last* `@` in the authority: a userinfo may percent-encode one, and the host is what
    // follows the final separator.
    match url[start..end].rfind('@') {
        Some(at) => Cow::Owned(format!("{}***@{}", &url[..start], &url[start + at + 1..])),
        None => Cow::Borrowed(url),
    }
}

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
            url: redacted(url).into_owned(),
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
        // Credentials in the URL are refused rather than stripped. Stripping would dial on
        // unauthenticated and leave the caller reading the peer's 401 for the reason; refusing
        // says it here. RFC 3986 §3.2.1 deprecates the form, and there is nowhere for it to go
        // anyway — RFC 9110 §7.2's `Host` admits no userinfo.
        if authority.as_str().contains('@') {
            return Err(refused(
                "it carries userinfo (RFC 3986 §3.2.1); authenticate with a request header, \
                 which this client keeps out of Debug, rather than in the URL",
            ));
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
        // Rebuilt from the parsed host and any explicit port rather than copied off the URL:
        // `Authority::as_str` includes userinfo, and this string becomes the `Host` header and
        // the peer named in every error. Built this way there is no byte to leak even if the
        // refusal above is one day relaxed.
        let travels = match authority.port_u16() {
            Some(explicit) => format!("{host}:{explicit}"),
            None => host.to_owned(),
        };
        let resource = uri
            .path_and_query()
            .map_or("/", |path| path.as_str())
            .to_owned();
        Ok(Self {
            secure,
            host: bare.to_owned(),
            authority: travels,
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
        // Nothing was sent, so no peer answered: there is no status to report.
        status: None,
        detail: error.to_string(),
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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

    /// RFC 3986 §3.2.1 userinfo is refused, and the refusal does not repeat the secret it
    /// refused — an error that quoted the URL back would be the leak in a different file.
    #[test]
    fn a_url_carrying_credentials_is_refused_without_repeating_them() {
        for url in [
            "wss://user:sekret@example.com:8443/v1",
            "wss://sekret@example.com/v1",
            "ws://user:sekret@localhost:9000/",
        ] {
            let error = Target::of(url).expect_err("refused");
            let WssError::Url { url: named, detail } = &error else {
                panic!("{url} must be refused as a URL, got: {error}")
            };
            assert!(detail.contains("userinfo"), "{detail}");
            assert!(!named.contains("sekret"), "the refusal named it: {named}");
            assert!(
                !format!("{error}").contains("sekret") && !format!("{error:?}").contains("sekret"),
                "neither Display nor Debug may carry it: {error} / {error:?}"
            );
        }
    }

    /// The authority that travels is rebuilt from the parsed host and port, so the `Host` header
    /// (RFC 9110 §7.2, which admits no userinfo) cannot inherit one from the URL.
    #[test]
    fn the_host_header_carries_the_authority_and_nothing_else() {
        let target = Target::of("wss://example.com:8443/v1").expect("parses");
        let upgrade =
            upgrade_request(&target, &WssRequest::new("wss://example.com:8443/v1")).expect("built");
        assert_eq!(
            upgrade.headers().get("host").expect("a Host"),
            "example.com:8443"
        );
        assert_eq!(upgrade.uri().to_string(), "wss://example.com:8443/v1");
    }

    /// Redaction is a property of the string, not of a successful parse: the URL an error most
    /// wants to quote is the malformed one, and it carries the secret just the same.
    #[test]
    fn redaction_replaces_userinfo_wherever_it_appears() {
        assert_eq!(
            redacted("wss://user:sekret@example.com/v1?k=v"),
            "wss://***@example.com/v1?k=v"
        );
        assert_eq!(
            redacted("wss://sekret@[::1]:9000/"),
            "wss://***@[::1]:9000/"
        );
        // Malformed enough that `Uri` refuses it, and still carrying the credential.
        assert_eq!(
            redacted("wss://user:sekret@ex ample/"),
            "wss://***@ex ample/"
        );
        // An `@` in the path is not userinfo and must survive untouched.
        assert_eq!(
            redacted("wss://example.com/mail/user@example.com"),
            "wss://example.com/mail/user@example.com"
        );
        // No scheme at all. Both of these parse as a `Uri`, so both reach the scheme refusal
        // with the credential still in the string — a pasted URL that lost its `wss://` is the
        // ordinary way to write one, and it must not travel into the error verbatim.
        assert_eq!(redacted("user:sekret@example.com"), "***@example.com");
        assert_eq!(
            redacted("//user:sekret@example.com/v1"),
            "//***@example.com/v1"
        );
        assert_eq!(redacted("sekret@example.com:8443"), "***@example.com:8443");
        // A relative path whose `@` is past the first separator is not an authority.
        assert_eq!(redacted("mail/user@example.com"), "mail/user@example.com");
        assert_eq!(redacted("wss://example.com/v1"), "wss://example.com/v1");
        assert_eq!(redacted("not a url"), "not a url");
    }

    /// The scheme-less forms reach the scheme refusal, so the refusal is where the credential
    /// would surface — `Display` included, which is what a caller logs.
    #[test]
    fn a_scheme_less_url_is_refused_without_repeating_its_credential() {
        for url in [
            "user:sekret@example.com",
            "//user:sekret@example.com/v1",
            "sekret@example.com:8443",
        ] {
            let error = Target::of(url).expect_err("refused");
            assert!(
                !format!("{error}").contains("sekret") && !format!("{error:?}").contains("sekret"),
                "{url} leaked: {error} / {error:?}"
            );
        }
    }
}
