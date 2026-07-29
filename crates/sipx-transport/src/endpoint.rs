//! The endpoint: one event loop driving the sans-IO core.
//!
//! Everything mutable lives in this loop — the transaction layer, the timer queue, the
//! sockets. No transaction is reachable from two tasks, so there are no locks in the
//! signalling path and no way to observe a half-applied transition. Applications talk to the
//! loop over channels.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use sipx_sip::transaction::{Dispatch, Output, Timer, TransactionKey, TransactionLayer, TuEvent};
use sipx_sip::{Header, HeaderName, Limits, Message, Request, Response, Timers, parse_datagram};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, Result};
use crate::nat::apply_received_and_rport;
use crate::target::{ConnectionKey, Target, TransportKind, response_destination};
use crate::tcp::{self, Pool, PoolConfig};
use crate::timers::TimerQueue;

/// How an endpoint is configured.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where to bind.
    pub bind: SocketAddr,
    /// The host to put in `Via` sent-by.
    ///
    /// Deliberately separate from the bind address: behind a NAT or a load balancer the two
    /// differ, and the socket's view is the wrong one to advertise.
    pub sent_by: String,
    /// The port to put in `Via` sent-by.
    ///
    /// `None` — and `Some(0)`, which means the same thing — is filled in with the port the
    /// socket actually got. Binding to port 0 asks the OS to choose one, and advertising the
    /// literal zero would tell peers to send responses to port 0.
    pub sent_by_port: Option<u16>,
    /// Transaction timer constants.
    pub timers: Timers,
    /// Parser limits.
    pub limits: Limits,
    /// How many events may queue for the application before new transactions are refused.
    pub capacity: usize,
    /// The largest datagram sipx will put on an unreliable transport.
    ///
    /// RFC 3261 §18.1.1 says a request approaching the path MTU must go over a
    /// congestion-controlled transport instead. Until sipx can switch transports mid-request —
    /// which changes the `Via` and therefore the transaction — it refuses with a named error
    /// rather than sending something that will be fragmented or silently truncated.
    pub mtu: usize,
    /// Whether to listen for TCP connections on the same port.
    pub tcp: bool,
    /// How sipx behaves as a TLS client, if TLS is to be used at all.
    #[cfg(feature = "tls")]
    pub tls_client: Option<crate::tls::ClientTls>,
    /// The identity sipx presents as a TLS server, and the port to listen on.
    ///
    /// A separate port from the cleartext one, because RFC 3261 §19.1.2 gives `sips` its own
    /// default (5061) and a peer connecting to 5060 does not expect a handshake.
    #[cfg(feature = "tls")]
    pub tls_server: Option<(crate::tls::ServerTls, u16)>,
    /// The port to listen for WebSocket connections on, if any.
    ///
    /// Its own port for the same reason TLS has one: a peer connecting to 5060 expects SIP on
    /// the wire, not an HTTP upgrade request.
    #[cfg(feature = "ws")]
    pub ws_server: Option<u16>,
    /// The identity sipx presents on the secure WebSocket port, and the port.
    #[cfg(feature = "wss")]
    pub wss_server: Option<(crate::tls::ServerTls, u16)>,
    /// How often to ping an otherwise idle WebSocket.
    ///
    /// Well under the idle timeout of the intermediaries that sit in front of browsers — most
    /// close a silent connection somewhere between 30 and 120 seconds, and a registration whose
    /// connection died silently is a phone that rings nowhere.
    #[cfg(feature = "ws")]
    pub ws_keepalive: std::time::Duration,
    /// How long a request may sit unanswered by the application before the transaction is
    /// abandoned.
    ///
    /// RFC 3261 §17.2 gives a server transaction in `Trying` or `Proceeding` no timer at all,
    /// because its model is that the transaction user always responds. Real applications do
    /// not, and a transaction nobody ever answers is held for the life of the process.
    ///
    /// Configurable because three minutes is not long for a telephone. A hunt group that rings
    /// for five before an agent picks up is ordinary, and an endpoint that abandoned the
    /// transaction at three would simply stop being able to answer such calls.
    pub unanswered_limit: std::time::Duration,
    /// How the connection pool behaves.
    pub pool: PoolConfig,
}

impl Config {
    /// A configuration bound to an address, advertising that same address.
    ///
    /// If the bind address names port 0, the advertised port is the one the socket is
    /// actually given. Note that binding to an unspecified address (`0.0.0.0`) leaves nothing
    /// sensible to advertise; set [`Config::sent_by`] explicitly in that case.
    #[must_use]
    pub fn new(bind: SocketAddr) -> Self {
        Self {
            bind,
            sent_by: bind.ip().to_string(),
            sent_by_port: None,
            timers: Timers::default(),
            limits: Limits::datagram(),
            capacity: 1024,
            mtu: 1300,
            tcp: true,
            #[cfg(feature = "tls")]
            tls_client: None,
            #[cfg(feature = "tls")]
            tls_server: None,
            #[cfg(feature = "ws")]
            ws_server: None,
            #[cfg(feature = "wss")]
            wss_server: None,
            #[cfg(feature = "ws")]
            ws_keepalive: std::time::Duration::from_secs(25),
            unanswered_limit: std::time::Duration::from_secs(180),
            pool: PoolConfig::default(),
        }
    }
}

/// A connection that finished its handshake and is ready to join the pool.
///
/// A closure rather than a stream: the pool lives on the driver's loop, and the three kinds of
/// handshake produce three unrelated stream types the loop has no reason to distinguish.
type Adopt = Box<dyn FnOnce(&mut Pool) + Send>;

/// A request that arrived and created a server transaction.
#[derive(Debug)]
pub struct Incoming {
    /// The transaction it belongs to; respond with [`Handle::respond`].
    pub key: TransactionKey,
    /// The request, with `received` and `rport` already applied to its topmost `Via`.
    pub request: Request,
    /// Where it came from.
    pub source: SocketAddr,
    /// How it arrived.
    pub transport: TransportKind,
}

/// Events from a client transaction: responses, then a terminal event.
#[derive(Debug)]
pub struct Responses {
    rx: mpsc::Receiver<TuEvent>,
    peeked: Option<TuEvent>,
}

impl Responses {
    /// The next event, or `None` once the transaction has finished.
    pub async fn next(&mut self) -> Option<TuEvent> {
        if let Some(event) = self.peeked.take() {
            return Some(event);
        }
        self.rx.recv().await
    }

    /// Look at the next event without consuming it.
    ///
    /// Used to decide whether a resolved candidate is viable before handing the stream to the
    /// caller, who must still see whatever was peeked at.
    pub async fn peek(&mut self) -> Option<&TuEvent> {
        if self.peeked.is_none() {
            self.peeked = self.rx.recv().await;
        }
        self.peeked.as_ref()
    }

    /// Wait for the first final response.
    ///
    /// Returns `None` if the transaction ended without one — a timeout or a transport error,
    /// both of which arrive as events on [`Self::next`] if the caller wants to tell them
    /// apart.
    pub async fn final_response(&mut self) -> Option<Response> {
        while let Some(event) = self.next().await {
            if let TuEvent::Response(response) = event
                && response.status.is_final()
            {
                return Some(*response);
            }
        }
        None
    }
}

#[derive(Debug)]
enum Command {
    Request {
        request: Box<Request>,
        target: Target,
        events: mpsc::Sender<TuEvent>,
        reply: oneshot::Sender<Result<TransactionKey>>,
    },
    Respond {
        key: TransactionKey,
        response: Box<Response>,
        /// Fired once the driver has performed the send, or with an error if there was no
        /// transaction left to send it on.
        sent: oneshot::Sender<Result<()>>,
    },
    /// A request handed straight to the transport, with no transaction behind it.
    Direct {
        request: Box<Request>,
        target: Target,
        /// Fired once the driver has actually performed the send.
        sent: oneshot::Sender<Result<()>>,
    },
    /// A keep-alive on a flow (RFC 5626 §4.4): a STUN Binding Request over UDP, a CRLFCRLF ping
    /// over anything connection-oriented.
    Keepalive {
        target: Target,
        /// Fired when the answer arrives: the reflexive address for STUN, `None` for a CRLF pong
        /// which carries no information beyond having arrived.
        answered: oneshot::Sender<Result<Option<SocketAddr>>>,
    },
    /// Install a sink for responses that match no client transaction.
    WatchUnmatched(mpsc::Sender<Unmatched>),
    /// How much state the driver is holding, for a soak test to assert on.
    Outstanding(oneshot::Sender<usize>),
    Shutdown,
}

/// What the endpoint has dropped because the application was not keeping up.
///
/// Kept as atomics behind an `Arc` rather than answered by the event loop, and that is the point:
/// the loop is busy in exactly the situation this counts, so a counter you could only read by
/// asking it would be unreadable when it mattered. [`Handle::shed`] reads it without touching the
/// loop at all.
///
/// The three kinds are counted apart because their consequences differ by an order of magnitude,
/// and one number would hide that.
#[derive(Debug, Default)]
struct Shed {
    requests: AtomicU64,
    acks: AtomicU64,
    unmatched: AtomicU64,
}

/// A response that matched no client transaction (RFC 3261 §16.7).
///
/// A user agent has nothing to do with one of these and is right to ignore it: it either answers a
/// request this endpoint did not send, or it arrived after its transaction was already gone. A
/// *forwarding element* is in the opposite position — §16.7 step 1 requires a stateful proxy that
/// finds no response context to "forward the response statelessly", which it cannot do if the
/// response never reaches it.
///
/// Delivered only to a caller that asked, through [`Handle::watch_unmatched`]. Nothing is allocated
/// and nothing changes for an endpoint that never asks.
#[derive(Debug, Clone)]
pub struct Unmatched {
    /// The response itself, unaltered.
    pub response: Response,
    /// Where it came from.
    pub source: SocketAddr,
    /// Which transport carried it.
    pub transport: TransportKind,
}

/// A snapshot of what an endpoint has shed (see [`Handle::shed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShedCounts {
    /// Requests that reached a server transaction and could not be handed to the application.
    ///
    /// Answered `503 Service Unavailable` with a `Retry-After`, so the peer is told something
    /// true rather than left to retransmit into a queue that is still full.
    pub requests: u64,
    /// **ACKs** that could not be handed over.
    ///
    /// The serious one, and the reason these are not one number. An ACK for a 2xx has no
    /// transaction to answer — RFC 3261 §17.1.1.3 makes it a new transaction of its own, and
    /// there is no response to an ACK in SIP at all — so there is no 503 to send and nothing
    /// retransmits it after Timer H. Both ends are then in a dialog that no timer will reap
    /// unless session timers (RFC 4028) happen to be in play. A non-zero count here means calls
    /// are leaking.
    pub acks: u64,
    /// Requests that matched no transaction and could not be handed over.
    ///
    /// The peer will retransmit an unmatched INVITE, so this is the most survivable of the three
    /// — but it is still loss, and it was previously invisible.
    pub unmatched: u64,
}

impl ShedCounts {
    /// Whether anything has been shed at all.
    #[must_use]
    pub fn any(self) -> bool {
        self.total() > 0
    }

    /// Everything shed, of every kind.
    #[must_use]
    pub fn total(self) -> u64 {
        self.requests
            .saturating_add(self.acks)
            .saturating_add(self.unmatched)
    }
}

/// A handle to a running endpoint.
#[derive(Debug, Clone)]
pub struct Handle {
    commands: mpsc::Sender<Command>,
    local_addr: SocketAddr,
    /// What has been dropped for backpressure, shared with the driver so it can be read while
    /// the driver is busy — which is the only time it is interesting.
    shed: Arc<Shed>,
    #[cfg(feature = "tls")]
    tls_addr: Option<SocketAddr>,
    #[cfg(feature = "ws")]
    ws_addr: Option<SocketAddr>,
    #[cfg(feature = "wss")]
    wss_addr: Option<SocketAddr>,
    /// The sent-by this endpoint uses on a WebSocket it dialled out (RFC 7118 §5.2).
    ///
    /// Invented once at bind time rather than per request: a `Via` that changed between a
    /// request and its retransmission would be a different `Via`.
    #[cfg(feature = "ws")]
    ws_sent_by: Arc<str>,
    sent_by: Arc<String>,
    sent_by_port: u16,
}

impl Handle {
    /// The address the endpoint is bound to.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The address the TLS listener is bound to, if one was configured.
    ///
    /// Needed because the TLS port may be 0 — "any" — and the caller cannot put a port it does
    /// not know into a `Contact`.
    #[cfg(feature = "tls")]
    #[must_use]
    pub fn tls_addr(&self) -> Option<SocketAddr> {
        self.tls_addr
    }

    /// The address the WebSocket listener is bound to, if one was configured.
    #[cfg(feature = "ws")]
    #[must_use]
    pub fn ws_addr(&self) -> Option<SocketAddr> {
        self.ws_addr
    }

    /// The address the secure WebSocket listener is bound to, if one was configured.
    #[cfg(feature = "wss")]
    #[must_use]
    pub fn wss_addr(&self) -> Option<SocketAddr> {
        self.wss_addr
    }

    /// The host and port this endpoint tells peers to reach it on.
    ///
    /// Not the same as [`Self::local_addr`], and the difference matters wherever an address
    /// goes into a message. An endpoint bound to `0.0.0.0` has a local address that means
    /// "everywhere" to us and nothing to a peer; behind a NAT the local address is private.
    /// `Contact` and `Via` must carry this.
    #[must_use]
    pub fn advertised(&self) -> String {
        format!("{}:{}", self.sent_by, self.sent_by_port)
    }

    /// Send a request, creating a client transaction.
    ///
    /// A `Via` is added if the request has none — the transport owns that header, since only
    /// it knows the branch and where responses should come back to.
    pub async fn send(&self, mut request: Request, target: Target) -> Result<Responses> {
        if request.headers.get(&HeaderName::Via).is_none() {
            let via = format!(
                "SIP/2.0/{} {};rport;branch={}",
                target.transport.as_str(),
                self.sent_by_for(target.transport),
                new_branch()
            );
            let header = Header::build(HeaderName::Via, Bytes::from(via))?;
            request.headers.push_front(header);
        }

        let (events_tx, events_rx) = mpsc::channel(32);
        let (reply_tx, reply_rx) = oneshot::channel();
        self.commands
            .send(Command::Request {
                request: Box::new(request),
                target,
                events: events_tx,
                reply: reply_tx,
            })
            .await
            .map_err(|_| Error::EndpointClosed)?;
        reply_rx.await.map_err(|_| Error::EndpointClosed)??;
        Ok(Responses {
            rx: events_rx,
            peeked: None,
        })
    }

    /// Send a request straight to the transport, with no transaction behind it.
    ///
    /// For the one request that has no transaction of its own: the ACK to a 2xx. RFC 3261
    /// §13.2.2.4 has it "passed to the transport layer directly for transmission", and it is
    /// the UAC core — not a transaction — that resends it when a retransmitted 2xx arrives.
    /// Putting it in a transaction instead earns it Timer E retransmissions toward a response
    /// that will never come, and a timeout 32 seconds later for a call that is up and talking.
    ///
    /// The `Via` is the caller's business here: an ACK for a 2xx carries a *new* branch
    /// (§13.2.2.4 makes it a new transaction as far as any proxy is concerned), and only the
    /// caller knows the dialog it belongs to.
    ///
    /// Returns once the bytes have been handed to the socket.
    pub async fn send_directly(&self, request: Request, target: Target) -> Result<()> {
        let (sent_tx, sent_rx) = oneshot::channel();
        self.commands
            .send(Command::Direct {
                request: Box::new(request),
                target,
                sent: sent_tx,
            })
            .await
            .map_err(|_| Error::EndpointClosed)?;
        sent_rx.await.map_err(|_| Error::EndpointClosed)?
    }

    /// Resolve a URI (RFC 3263) and send to the resulting candidates in order.
    ///
    /// A candidate that fails is not the request failing — the next one is tried, and only an
    /// exhausted list is an error. Each attempt is its own transaction with its own branch,
    /// which is what makes retrying legal: a transaction is bound to the destination it was
    /// created for.
    ///
    /// Note what "fails" costs on an unreliable transport. A dead TCP peer refuses the
    /// connection and is known bad in milliseconds; a dead UDP peer says nothing at all, and
    /// the only way to learn it is dead is to let the transaction time out — 64·T1, or 32
    /// seconds with the default constants. That is a property of UDP, not of this function,
    /// but it means a long candidate list over UDP is slow to exhaust. Callers that cannot
    /// afford it should use [`Handle::send`] with a candidate list they manage themselves.
    pub async fn send_to_uri<R: crate::resolve::Resolver + ?Sized>(
        &self,
        request: Request,
        uri: &sipx_sip::Uri,
        resolver: &R,
    ) -> Result<Responses> {
        let candidates = crate::resolve::resolve(uri, resolver, &mut crate::resolve::OsRng);
        if candidates.is_empty() {
            return Err(Error::Unresolvable(uri.to_bytes().to_vec()));
        }

        let mut last = Err(Error::Unresolvable(uri.to_bytes().to_vec()));
        for target in candidates {
            let mut responses = self.send(request.clone(), target).await?;
            // Peek at the first event. A transport error here means this candidate is dead;
            // anything else means the exchange has begun and belongs to the caller.
            match responses.peek().await {
                // Both are "this candidate is dead". A transport error says so directly; a
                // timeout is how UDP says it, since a black hole sends nothing back.
                Some(TuEvent::TransportError) => last = Err(Error::EndpointClosed),
                Some(TuEvent::Timeout) => {
                    last = Err(Error::Unresolvable(uri.to_bytes().to_vec()));
                }
                _ => return Ok(responses),
            }
        }
        last
    }

    /// The host and port this endpoint tells peers to reach it on over this transport.
    ///
    /// Almost always its real host and port, as [`Self::advertised`] gives them. The exception
    /// is a WebSocket sipx dialled out on: RFC 7118 §5.2 says such a client has no listening
    /// port and must invent an unresolvable name, and advertising a real address instead would
    /// send a proxy off to a port that is not listening while the connection it should have
    /// used sits open. An endpoint that *does* listen for WebSocket connections is not that
    /// client, and keeps its own name.
    ///
    /// Belongs in a `Contact` as much as in a `Via`, for the same reason: both are answers to
    /// "where do I reach you".
    #[must_use]
    pub fn sent_by_for(&self, transport: TransportKind) -> String {
        #[cfg(feature = "ws")]
        if matches!(transport, TransportKind::Ws | TransportKind::Wss) && !self.listens_for_ws() {
            return self.ws_sent_by.to_string();
        }
        // TLS is listened for on a port of its own (RFC 3261 §19.1.2), so a sent-by naming the
        // cleartext port would direct any response that cannot reuse the connection at a port
        // speaking a different protocol.
        #[cfg(feature = "tls")]
        if matches!(transport, TransportKind::Tls)
            && let Some(addr) = self.tls_addr
        {
            return format!("{}:{}", self.sent_by, addr.port());
        }
        let _ = transport;
        format!("{}:{}", self.sent_by, self.sent_by_port)
    }

    #[cfg(feature = "ws")]
    fn listens_for_ws(&self) -> bool {
        #[cfg(feature = "wss")]
        if self.wss_addr.is_some() {
            return true;
        }
        self.ws_addr.is_some()
    }

    /// Send a response on a server transaction.
    ///
    /// Returns once the response has been handed to the socket, not merely queued. The
    /// difference is invisible until a process answers a call and exits — then the queued
    /// version loses the response to the exit, and the caller sees a timeout for a call that
    /// was in fact refused. Every caller already assumed this; now it is true.
    pub async fn respond(&self, key: &TransactionKey, response: Response) -> Result<()> {
        let (sent, delivered) = oneshot::channel();
        self.commands
            .send(Command::Respond {
                key: key.clone(),
                response: Box::new(response),
                sent,
            })
            .await
            .map_err(|_| Error::EndpointClosed)?;
        delivered.await.map_err(|_| Error::EndpointClosed)?
    }

    /// Keep a flow alive, and wait for the answer (RFC 5626 §4.4).
    ///
    /// Over UDP this is a STUN Binding Request (§4.4.2) and the answer carries the reflexive
    /// address the far end saw — which is the reason to prefer STUN over a SIP request: §4.4.2 has
    /// a *changed* mapped address mean the flow has failed, so the keep-alive detects a NAT
    /// rebinding rather than only proving the socket still works. Over anything
    /// connection-oriented it is §4.4.1's CRLFCRLF ping, and the pong carries nothing but its own
    /// arrival, so the answer is `None`.
    ///
    /// `within` is how long to wait. §4.4.1 sets it at 10 seconds for the CRLF technique and
    /// requires a UA whose pong does not arrive to "treat the flow as failed"; the number is the
    /// caller's because it is RFC 5626 policy rather than a property of the transport.
    ///
    /// Sent over the same connection a request would take, which is the whole point: a ping on a
    /// second connection proves a flow nobody is using.
    pub async fn keepalive(
        &self,
        target: Target,
        within: std::time::Duration,
    ) -> Result<Option<SocketAddr>> {
        let (answered_tx, answered_rx) = oneshot::channel();
        self.commands
            .send(Command::Keepalive {
                target,
                answered: answered_tx,
            })
            .await
            .map_err(|_| Error::EndpointClosed)?;
        match tokio::time::timeout(within, answered_rx).await {
            Ok(Ok(result)) => result,
            // The driver dropped the waiter, which happens only on shutdown.
            Ok(Err(_)) => Err(Error::EndpointClosed),
            // §4.4.1: no answer in time is a failed flow, not a slow one.
            Err(_) => Err(Error::KeepaliveUnanswered),
        }
    }

    /// Watch for responses that match no client transaction (RFC 3261 §16.7).
    ///
    /// Opt-in, and the reason it is opt-in is the whole design: a user agent has no answer for one
    /// of these — it either answers a request this endpoint did not send, or it arrived after its
    /// transaction was gone — and should not have to handle a case it cannot act on. A forwarding
    /// element is required to act on it, so it asks.
    ///
    /// Until someone calls this, unmatched responses are logged and dropped exactly as before, and
    /// no channel exists to allocate into.
    ///
    /// Calling it twice **replaces** the sink. Two watchers would each see some of the responses
    /// and neither would see all of them, which is a subtler failure than having none.
    pub async fn watch_unmatched(&self, capacity: usize) -> Result<mpsc::Receiver<Unmatched>> {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        self.commands
            .send(Command::WatchUnmatched(tx))
            .await
            .map_err(|_| Error::EndpointClosed)?;
        Ok(rx)
    }

    /// What this endpoint has dropped because the application was not keeping up.
    ///
    /// Read straight from a shared counter rather than by asking the event loop, because the loop
    /// is busy in precisely the situation this counts. A metric that is unavailable exactly when
    /// it is interesting is not a metric.
    ///
    /// Non-zero is not automatically a fault — shedding under load is a policy, and a `503` tells
    /// a peer something true. `ShedCounts::acks` is different: see its documentation.
    #[must_use]
    pub fn shed(&self) -> ShedCounts {
        ShedCounts {
            requests: self.shed.requests.load(Ordering::Relaxed),
            acks: self.shed.acks.load(Ordering::Relaxed),
            unmatched: self.shed.unmatched.load(Ordering::Relaxed),
        }
    }

    /// How many transactions and destinations the endpoint is still holding.
    ///
    /// Exposed for the soak test in `sipx-testkit`, and worth exposing: a transaction store
    /// that leaks is a slow, quiet outage — the stack goes on working for hours and then stops,
    /// and by then the cause is a long way behind. This is the cheapest way to notice.
    ///
    /// Note what a *non-zero* answer does not mean. RFC 3261 §17 keeps a completed transaction
    /// for Timer J, thirty-two seconds, so it can absorb a retransmission. Sampling before that
    /// has elapsed counts the specification.
    pub async fn outstanding(&self) -> Result<usize> {
        let (tx, rx) = oneshot::channel();
        self.commands
            .send(Command::Outstanding(tx))
            .await
            .map_err(|_| Error::EndpointClosed)?;
        rx.await.map_err(|_| Error::EndpointClosed)
    }

    /// Stop the endpoint.
    pub async fn shutdown(&self) {
        let _ = self.commands.send(Command::Shutdown).await;
    }
}

/// A `branch` token: the RFC's magic cookie plus 64 bits from a cryptographic RNG.
///
/// The width is ours, not the RFC's. A guessable branch lets an off-path attacker inject
/// responses into a transaction, so this is not a place for a counter.
#[must_use]
pub fn new_branch() -> String {
    use rand::Rng;
    let value: u64 = rand::rng().random();
    format!("z9hG4bK{value:016x}")
}

/// Bind an endpoint and start its loop.
///
/// Returns a handle for sending, and a receiver of the requests that arrive.
pub async fn bind(config: Config) -> Result<(Handle, mpsc::Receiver<Incoming>)> {
    let (socket, listener, local_addr) = bind_matching_ports(&config).await?;
    // Port 0 in the configuration means the same as absent: it is a request for any port,
    // not an advertisement of port zero.
    let sent_by_port = match config.sent_by_port {
        Some(port) if port != 0 => port,
        _ => local_addr.port(),
    };

    // One channel for every handshaked connection, whatever kind it is. The driver owns the
    // pool, so adoption has to happen on its loop; what joins is a closure rather than a stream
    // because TCP-over-TLS, WebSocket and WebSocket-over-TLS are three unrelated types and the
    // loop has no reason to know which it is holding. One channel is also one `select!` branch,
    // which matters more than it looks: `tokio::select!` cannot compile a branch out behind a
    // feature flag, so a branch per optional transport does not build with that feature off.
    let (adopt_tx, adopt_rx) = mpsc::channel::<Adopt>(64);

    #[cfg(feature = "tls")]
    let secure_addr = match config.tls_server.clone() {
        Some((server, port)) => Some(listen_tls(config.bind.ip(), port, server, &adopt_tx).await?),
        None => None,
    };
    #[cfg(feature = "ws")]
    let upgrade_addr = match config.ws_server {
        Some(port) => {
            Some(listen_ws(config.bind.ip(), port, config.ws_keepalive, &adopt_tx).await?)
        }
        None => None,
    };
    #[cfg(feature = "wss")]
    let secure_upgrade_addr = match config.wss_server.clone() {
        Some((server, port)) => Some(
            listen_wss(
                config.bind.ip(),
                port,
                server,
                config.ws_keepalive,
                &adopt_tx,
            )
            .await?,
        ),
        None => None,
    };

    let (commands_tx, commands_rx) = mpsc::channel(config.capacity);
    let (incoming_tx, incoming_rx) = mpsc::channel(config.capacity);

    let shed = Arc::new(Shed::default());
    let handle = Handle {
        commands: commands_tx,
        local_addr,
        shed: Arc::clone(&shed),
        #[cfg(feature = "tls")]
        tls_addr: secure_addr,
        #[cfg(feature = "ws")]
        ws_addr: upgrade_addr,
        #[cfg(feature = "wss")]
        wss_addr: secure_upgrade_addr,
        #[cfg(feature = "ws")]
        ws_sent_by: Arc::from(crate::ws::invented_sent_by()),
        sent_by: Arc::new(config.sent_by.clone()),
        sent_by_port,
    };

    let (net_tx, net_rx) = mpsc::channel(config.capacity);
    let (accept_tx, accept_rx) = mpsc::channel(64);
    if let Some(listener) = listener {
        tokio::spawn(tcp::accept_loop(listener, accept_tx));
    }

    let driver = Driver {
        socket: Arc::new(socket),
        layer: TransactionLayer::new(config.timers),
        timers: TimerQueue::new(),
        destinations: HashMap::new(),
        handed_over: HashMap::new(),
        reconnect: HashMap::new(),
        unanswered_limit: config.unanswered_limit,
        clients: HashMap::new(),
        incoming: incoming_tx,
        commands: commands_rx,
        net: net_rx,
        accepts: accept_rx,
        adopts: adopt_rx,
        _adopt: adopt_tx,
        #[cfg(feature = "tls")]
        tls_client: config.tls_client.clone(),
        #[cfg(feature = "ws")]
        ws_keepalive: config.ws_keepalive,
        pool: Pool::new(config.pool, config.limits, net_tx),
        limits: config.limits,
        mtu: config.mtu,
        shed,
        unmatched: None,
        stun_waiters: HashMap::new(),
        pong_waiters: HashMap::new(),
    };
    tokio::spawn(driver.run());

    Ok((handle, incoming_rx))
}

/// Listen for TLS connections, handshaking each off the accept path.
///
/// Off the accept path so one slow or hostile peer cannot hold up every other connection
/// waiting behind it. The listener's own address is returned because the caller may have asked
/// for port 0 and cannot put a port it does not know into a `Contact`.
#[cfg(feature = "tls")]
async fn listen_tls(
    ip: std::net::IpAddr,
    port: u16,
    server: crate::tls::ServerTls,
    adopt: &mpsc::Sender<Adopt>,
) -> Result<SocketAddr> {
    let listener = TcpListener::bind(SocketAddr::new(ip, port)).await?;
    let addr = listener.local_addr()?;
    let (raw_tx, mut raw_rx) = mpsc::channel(64);
    tokio::spawn(tcp::accept_loop(listener, raw_tx));

    let adopt = adopt.clone();
    tokio::spawn(async move {
        while let Some((stream, peer)) = raw_rx.recv().await {
            let acceptor = server.acceptor();
            let adopt = adopt.clone();
            tokio::spawn(async move {
                match acceptor.accept(stream).await {
                    Ok(tls) => {
                        let _ = adopt
                            .send(Box::new(move |pool: &mut Pool| pool.accept_tls(tls, peer)))
                            .await;
                    }
                    Err(error) => tracing::debug!(%error, %peer, "inbound TLS handshake failed"),
                }
            });
        }
    });
    Ok(addr)
}

/// Listen for WebSocket connections, upgrading each off the accept path.
#[cfg(feature = "ws")]
async fn listen_ws(
    ip: std::net::IpAddr,
    port: u16,
    keepalive: std::time::Duration,
    adopt: &mpsc::Sender<Adopt>,
) -> Result<SocketAddr> {
    let listener = TcpListener::bind(SocketAddr::new(ip, port)).await?;
    let addr = listener.local_addr()?;
    let (raw_tx, mut raw_rx) = mpsc::channel(64);
    tokio::spawn(tcp::accept_loop(listener, raw_tx));

    let adopt = adopt.clone();
    tokio::spawn(async move {
        while let Some((stream, peer)) = raw_rx.recv().await {
            let adopt = adopt.clone();
            tokio::spawn(async move {
                adopt_upgraded(
                    crate::ws::accept(stream, peer).await,
                    peer,
                    TransportKind::Ws,
                    keepalive,
                    &adopt,
                )
                .await;
            });
        }
    });
    Ok(addr)
}

/// Listen for secure WebSocket connections: TLS, then the upgrade.
///
/// The certificate policy is `T-7`'s because this is `T-7`'s code — the same acceptor, built
/// from the same [`crate::tls::ServerTls`]. A second implementation of a security check is how
/// one of the two ends up weaker.
#[cfg(feature = "wss")]
async fn listen_wss(
    ip: std::net::IpAddr,
    port: u16,
    server: crate::tls::ServerTls,
    keepalive: std::time::Duration,
    adopt: &mpsc::Sender<Adopt>,
) -> Result<SocketAddr> {
    let listener = TcpListener::bind(SocketAddr::new(ip, port)).await?;
    let addr = listener.local_addr()?;
    let (raw_tx, mut raw_rx) = mpsc::channel(64);
    tokio::spawn(tcp::accept_loop(listener, raw_tx));

    let adopt = adopt.clone();
    tokio::spawn(async move {
        while let Some((stream, peer)) = raw_rx.recv().await {
            let acceptor = server.acceptor();
            let adopt = adopt.clone();
            tokio::spawn(async move {
                let tls = match acceptor.accept(stream).await {
                    Ok(tls) => tls,
                    Err(error) => {
                        tracing::debug!(%error, %peer, "inbound WSS handshake failed");
                        return;
                    }
                };
                adopt_upgraded(
                    crate::ws::accept(tls, peer).await,
                    peer,
                    TransportKind::Wss,
                    keepalive,
                    &adopt,
                )
                .await;
            });
        }
    });
    Ok(addr)
}

/// Hand a completed WebSocket upgrade to the driver, or report why there was none.
#[cfg(feature = "ws")]
async fn adopt_upgraded<S>(
    upgraded: std::result::Result<crate::ws::Socket<S>, crate::ws::WsError>,
    peer: SocketAddr,
    transport: TransportKind,
    keepalive: std::time::Duration,
    adopt: &mpsc::Sender<Adopt>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    match upgraded {
        Ok(socket) => {
            let key = ConnectionKey::new(peer, transport);
            let _ = adopt
                .send(Box::new(move |pool: &mut Pool| {
                    pool.accept_ws(socket, key, keepalive);
                }))
                .await;
        }
        Err(error) => tracing::debug!(%error, %peer, "inbound websocket handshake failed"),
    }
}

/// Bind UDP and TCP to the *same* port.
///
/// Peers assume they are the same: a `Via` naming `SIP/2.0/TCP host:port` and one naming UDP
/// refer to one port number, and an endpoint whose two transports live on different ports is
/// unreachable over one of them.
///
/// The awkward part is that UDP and TCP have independent port spaces, so a port the OS hands
/// out for UDP may already be held by someone else for TCP. When the caller asked for port 0 —
/// "any port" — that is not an error, it is a port to not use: try again. When the caller named
/// a port, it is a real conflict and is reported as one.
async fn bind_matching_ports(
    config: &Config,
) -> Result<(UdpSocket, Option<TcpListener>, SocketAddr)> {
    const ATTEMPTS: usize = 16;

    let wants_any_port = config.bind.port() == 0;
    let mut last_error = None;

    for _ in 0..ATTEMPTS {
        let socket = UdpSocket::bind(config.bind).await?;
        let local_addr = socket.local_addr()?;

        if !config.tcp {
            return Ok((socket, None, local_addr));
        }

        match TcpListener::bind(local_addr).await {
            Ok(listener) => return Ok((socket, Some(listener), local_addr)),
            Err(error) if wants_any_port && error.kind() == std::io::ErrorKind::AddrInUse => {
                // Someone else holds this port for TCP. Drop the UDP socket so the OS may
                // hand the port out again, and ask for another.
                drop(socket);
                last_error = Some(error);
            }
            Err(error) => return Err(error.into()),
        }
    }

    Err(last_error
        .unwrap_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "no port was free for both UDP and TCP",
            )
        })
        .into())
}

struct Driver {
    socket: Arc<UdpSocket>,
    layer: TransactionLayer,
    timers: TimerQueue<(TransactionKey, Timer)>,
    destinations: HashMap<TransactionKey, Target>,
    /// When each server transaction was handed to the application, so one it never answers can
    /// be abandoned rather than held for the life of the process.
    handed_over: HashMap<TransactionKey, tokio::time::Instant>,
    /// Where a response goes if the connection its request arrived on has closed.
    ///
    /// RFC 3261 §18.2.2: the address from `received` at the `sent-by` port, which is a port the
    /// peer listens on — unlike the source port, which is the ephemeral one it dialled out
    /// from. Held only for server transactions on a connection-oriented transport, because it
    /// is the only case where the question arises.
    reconnect: HashMap<TransactionKey, Target>,
    /// How long a request may sit unanswered before its transaction is abandoned.
    unanswered_limit: std::time::Duration,
    clients: HashMap<TransactionKey, mpsc::Sender<TuEvent>>,
    incoming: mpsc::Sender<Incoming>,
    commands: mpsc::Receiver<Command>,
    net: mpsc::Receiver<tcp::Event>,
    accepts: mpsc::Receiver<(tokio::net::TcpStream, SocketAddr)>,
    adopts: mpsc::Receiver<Adopt>,
    /// Held only to keep the adoption channel open when no optional listener is configured. A
    /// closed channel would leave that `select!` branch resolving instantly on every pass.
    _adopt: mpsc::Sender<Adopt>,
    #[cfg(feature = "tls")]
    tls_client: Option<crate::tls::ClientTls>,
    #[cfg(feature = "ws")]
    ws_keepalive: std::time::Duration,
    pool: Pool,
    limits: Limits,
    mtu: usize,
    /// Keep-alives sent over UDP, waiting for the STUN response that answers them.
    /// Shared with every [`Handle`]; see [`ShedCounts`].
    shed: Arc<Shed>,
    /// Where to send responses that match no client transaction, if anyone asked for them.
    ///
    /// `None` is the ordinary case and costs nothing: no channel exists, and the response is
    /// logged and dropped exactly as before.
    unmatched: Option<mpsc::Sender<Unmatched>>,
    stun_waiters: HashMap<crate::stun::TransactionId, oneshot::Sender<Result<Option<SocketAddr>>>>,
    /// Keep-alives sent over a connection, waiting for a CRLF pong.
    ///
    /// A queue per connection rather than one slot: nothing stops a caller pinging twice, and
    /// pongs are indistinguishable from each other, so the only honest match is first-in-first-out.
    pong_waiters: HashMap<
        ConnectionKey,
        std::collections::VecDeque<oneshot::Sender<Result<Option<SocketAddr>>>>,
    >,
}

/// Proof that [`Endpoint::perform`] ran to completion.
///
/// It exists so that "the datagram is on the wire before the caller is told so" is a property of
/// the *types* rather than of the order two statements happen to be written in. `respond` reports
/// success by consuming this value, so moving the report above the send is a compile error rather
/// than a silent regression.
///
/// `X-36` is why. The test named `respond_returns_only_once_the_response_has_been_sent` could not
/// detect the reversal: on a `current_thread` runtime, sending on the oneshot does not yield, so
/// `perform` completed before the waiting task was ever polled — the datagram was always out by
/// the time anyone could look, whichever order the two lines were in. A test cannot observe the
/// difference, so the guarantee is made structural instead.
struct Performed;

impl Performed {
    /// The success `respond` reports, obtainable only from proof that the send happened.
    ///
    /// Clippy objects to both halves of this signature, and both are the point. `unused_self`: taking
    /// `self` by value is the entire mechanism — it is what makes the `Ok` unobtainable without the
    /// send. `unnecessary_wraps`: the `Result` is what goes over the oneshot, whose other arm really
    /// can be `Err(Error::NoTransaction)`, so the wrap is the caller's type and not decoration.
    #[allow(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "consuming self is the guarantee; the Result is the channel's type"
    )]
    fn into_result(self) -> Result<()> {
        Ok(())
    }
}

impl Driver {
    async fn run(mut self) {
        let mut buf = vec![0u8; 65_536];
        // Idle connections are swept periodically rather than given a timer each; the pool is
        // small and the sweep is cheap.
        let mut idle_sweep = tokio::time::interval(std::time::Duration::from_secs(30));
        idle_sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let deadline = self.timers.next_deadline();
            tokio::select! {
                received = self.socket.recv_from(&mut buf) => match received {
                    Ok((len, source)) => {
                        let datagram = Bytes::copy_from_slice(buf.get(..len).unwrap_or(&[]));
                        self.on_datagram(datagram, source).await;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "receive failed");
                    }
                },
                () = sleep_until(deadline), if deadline.is_some() => {
                    self.on_timers().await;
                }
                command = self.commands.recv() => match command {
                    Some(Command::Shutdown) | None => return,
                    Some(command) => self.on_command(command).await,
                },
                Some(event) = self.net.recv() => self.on_net_event(event).await,
                Some((stream, peer)) = self.accepts.recv() => self.pool.accept(stream, peer),
                Some(adopt) = self.adopts.recv() => adopt(&mut self.pool),
                _ = idle_sweep.tick() => {
                    for closed in self.pool.evict_idle() {
                        tracing::debug!(peer = %closed.peer, "closed an idle connection");
                    }
                    self.abandon_unanswered();
                }
            }
        }
    }

    async fn on_datagram(&mut self, datagram: Bytes, source: SocketAddr) {
        // RFC 5389 §7.3's test, before the SIP parser sees it: a STUN response is not a SIP
        // message and would be dropped as malformed, taking the keep-alive with it.
        if crate::stun::is_stun(&datagram) {
            self.on_stun(&datagram, source);
            return;
        }
        match parse_datagram(datagram, &self.limits) {
            Ok(message) => self.on_message(message, source, TransportKind::Udp).await,
            Err(error) => {
                // One malformed packet must not disturb the socket. The alternative is a
                // trivial denial of service.
                tracing::debug!(%error, %source, "dropping malformed datagram");
            }
        }
    }

    /// Send one keep-alive and remember who is waiting for the answer (RFC 5626 §4.4).
    async fn on_keepalive(
        &mut self,
        target: Target,
        answered: oneshot::Sender<Result<Option<SocketAddr>>>,
    ) {
        // Waiters whose caller has given up. Swept here rather than on a timer: the only thing
        // that creates them is this method, so this is the only place the map can grow.
        self.stun_waiters.retain(|_, waiter| !waiter.is_closed());
        self.pong_waiters.retain(|_, queue| {
            queue.retain(|waiter| !waiter.is_closed());
            !queue.is_empty()
        });

        if target.transport == TransportKind::Udp {
            // §4.4.2: STUN for UDP flows. The transaction ID is what ties the response back, and
            // §6 of RFC 5389 wants it unguessable — a forged response naming a different mapped
            // address would have a UA declare a working flow dead.
            let id = crate::stun::new_transaction_id();
            let request = Bytes::from(crate::stun::binding_request(&id));
            match self.transmit_raw(request, &target).await {
                Ok(()) => {
                    self.stun_waiters.insert(id, answered);
                }
                Err(error) => {
                    let _ = answered.send(Err(error));
                }
            }
            return;
        }

        // §4.4.1: CRLFCRLF is the ping, and the pong is a lone CRLF the peer's parser is
        // otherwise told to ignore.
        let key = target.connection();
        match self
            .transmit_raw(Bytes::from_static(b"\r\n\r\n"), &target)
            .await
        {
            Ok(()) => {
                self.pong_waiters
                    .entry(key)
                    .or_default()
                    .push_back(answered);
            }
            Err(error) => {
                let _ = answered.send(Err(error));
            }
        }
    }

    /// Answer the waiter a STUN reply belongs to (RFC 5626 §4.4.2).
    fn on_stun(&mut self, datagram: &[u8], source: SocketAddr) {
        let Some(reply) = crate::stun::parse_reply(datagram) else {
            // A Binding *Request*: something on the network is treating this socket as a STUN
            // server. Not ours to answer, and not an error worth raising.
            tracing::debug!(%source, "ignoring a STUN message that is not a reply");
            return;
        };
        let Some(waiter) = self.stun_waiters.remove(&reply.id()) else {
            // An unsolicited or late reply. Dropping it is right: matching it to a *different*
            // keep-alive would report one flow's liveness as another's.
            tracing::debug!(%source, "a STUN reply matched no keep-alive");
            return;
        };
        let answer = match reply {
            crate::stun::Reply::Bound { mapped, .. } => Ok(mapped),
            // §4.4.2: "If a STUN Binding Error Response is received ... the UA considers the flow
            // failed."
            crate::stun::Reply::Failed { .. } => Err(Error::KeepaliveRefused),
        };
        let _ = waiter.send(answer);
    }

    async fn on_net_event(&mut self, event: tcp::Event) {
        match event {
            tcp::Event::Message {
                message,
                source,
                transport,
            } => {
                self.on_message(*message, source, transport).await;
            }
            tcp::Event::Pong { key } => {
                // First waiter for this connection, or nobody — a peer is entitled to send a
                // CRLF we did not ask for, and RFC 3261 §7.5 says to ignore it.
                if let Some(queue) = self.pong_waiters.get_mut(&key)
                    && let Some(waiter) = queue.pop_front()
                {
                    let _ = waiter.send(Ok(None));
                }
            }
            tcp::Event::Closed { key } => {
                self.pool.remove(&key);
                // A flow whose connection has gone is a failed flow, and saying so now beats
                // making the caller wait out its own timeout for something already known.
                if let Some(queue) = self.pong_waiters.remove(&key) {
                    for waiter in queue {
                        let _ = waiter.send(Err(Error::ConnectionClosed));
                    }
                }
                self.fail_transactions_on(&key).await;
            }
        }
    }

    /// Fail every transaction bound to a connection that has gone.
    ///
    /// The alternative is letting them time out, which means waiting up to 32 seconds to
    /// discover something already known — a bad experience and a resource leak.
    async fn fail_transactions_on(&mut self, closed: &ConnectionKey) {
        let affected: Vec<TransactionKey> = self
            .destinations
            .iter()
            .filter(|(_, target)| &target.connection() == closed)
            // A server transaction that knows where the peer listens is not failed by the loss
            // of the connection its request arrived on: RFC 3261 §18.2.2 has it open a new one
            // to the advertised port, and the response is still deliverable.
            .filter(|(key, _)| !self.reconnect.contains_key(*key))
            .map(|(key, _)| key.clone())
            .collect();
        for key in affected {
            let outputs = self.layer.on_transport_error(&key);
            self.perform(&key, outputs, None).await;
        }
    }

    async fn on_message(&mut self, message: Message, source: SocketAddr, transport: TransportKind) {
        let message = match message {
            Message::Request(mut request) => {
                apply_received_and_rport(&mut request, source);
                Message::Request(request)
            }
            response @ Message::Response(_) => response,
        };

        // A server transaction's responses go wherever its topmost Via says, which is why the
        // destination is computed now, from the request as amended above.
        // RFC 5923: on a connection-oriented transport the response goes back over the
        // connection the request arrived on, before §18.2.2 is consulted at all. Opening a new
        // connection to a NATed client's `Via` cannot work.
        let advertised = match &message {
            Message::Request(request) => request
                .headers
                .typed::<sipx_sip::headers::Via>()
                .and_then(std::result::Result::ok)
                .map(|via| response_destination(&via, source, transport)),
            Message::Response(_) => None,
        };
        let reply_to = match &message {
            Message::Request(_) if transport == TransportKind::Udp => advertised
                .clone()
                .unwrap_or_else(|| Target::new(source, transport)),
            _ => Target::new(source, transport),
        };

        match self.layer.receive(message, transport.reliability()) {
            Dispatch::Created { key, outputs } => {
                self.destinations.insert(key.clone(), reply_to);
                self.handed_over
                    .insert(key.clone(), tokio::time::Instant::now());
                // §18.2.2's fallback only arises on a transport that has a connection to lose.
                if transport.reliability().is_reliable()
                    && let Some(advertised) = advertised
                {
                    self.reconnect.insert(key.clone(), advertised);
                }
                self.perform(&key, outputs, Some((source, transport))).await;
            }
            Dispatch::Matched { key, outputs } => {
                self.perform(&key, outputs, Some((source, transport))).await;
            }
            Dispatch::Unmatched(message) => {
                tracing::debug!(%source, "message matched no transaction");
                // A response that matched nothing. RFC 3261 §16.7 step 1 makes this a forwarding
                // element's business — it must forward such a response statelessly — and no
                // business at all of a user agent, which is why it goes only to a caller that
                // asked for it.
                if let Message::Response(response) = &*message {
                    if let Some(sink) = &self.unmatched {
                        // `try_send`, and a full watcher's channel is its own problem: blocking
                        // the loop here would stop every timer in the endpoint to wait for a
                        // consumer that is already behind.
                        if sink
                            .try_send(Unmatched {
                                response: response.clone(),
                                source,
                                transport,
                            })
                            .is_err()
                        {
                            self.shed.unmatched.fetch_add(1, Ordering::Relaxed);
                            tracing::warn!(
                                %source,
                                "unmatched-response watcher is not keeping up; dropped one"
                            );
                        }
                    }
                    return;
                }
                // An unmatched ACK belongs to the application; anything else is noise it can
                // still choose to look at.
                if let Message::Request(request) = *message {
                    let Some(key) = TransactionKey::from_request(&request) else {
                        return;
                    };
                    let method = request.method.clone();
                    if self
                        .incoming
                        .try_send(Incoming {
                            key,
                            request,
                            source,
                            transport,
                        })
                        .is_err()
                    {
                        // Previously `let _ = …`: the request was gone, nothing was logged, and no
                        // counter moved. There is no transaction here to answer with a 503 — this
                        // request matched none — so counting and saying so is the whole of what
                        // can be done, and it is a great deal more than nothing.
                        self.shed.unmatched.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            %source,
                            method = %method,
                            "application queue full; an unmatched request was dropped"
                        );
                    }
                }
            }
        }
    }

    /// Drop server transactions the application never answered.
    ///
    /// RFC 3261 §17.2 gives a server transaction in `Trying` no timer at all, because its model
    /// is that the transaction user always responds. Real applications do not: one that ignores
    /// a method it does not implement, or that panics in a handler, leaves the transaction
    /// there — and nothing ever collects it, so the store grows for as long as traffic arrives.
    /// A soak run found exactly this: 300 of them for 300 calls, still present two minutes on.
    ///
    /// The bound is generous on purpose. A request may legitimately take a long time to answer
    /// — a call that rings for a minute is an unanswered INVITE the whole time — so this is a
    /// backstop against *never*, not a deadline.
    fn abandon_unanswered(&mut self) {
        let now = tokio::time::Instant::now();
        let stale: Vec<TransactionKey> = self
            .handed_over
            .iter()
            .filter(|(_, at)| now.saturating_duration_since(**at) > self.unanswered_limit)
            .map(|(key, _)| key.clone())
            .collect();

        for key in stale {
            self.handed_over.remove(&key);

            // What is being abandoned, named. A warning that blames the application and then
            // says nothing about which request, which method or which peer leaves an operator
            // with N identical lines and nowhere to start.
            let described = self.layer.server_request(&key).map(|request| {
                (
                    request.method.clone(),
                    request
                        .headers
                        .value(&HeaderName::CallId)
                        .map(|id| String::from_utf8_lossy(&id).into_owned())
                        .unwrap_or_default(),
                )
            });

            if !self.layer.abandon(&key) {
                continue;
            }
            if let Some((method, call_id)) = described {
                tracing::warn!(
                    ?method,
                    %call_id,
                    limit = ?self.unanswered_limit,
                    "abandoning a transaction the application never answered; that is an \
                     application bug rather than a network one"
                );
            }

            // `clients` is never touched, and `destinations` only when nothing else claims the
            // key. A `TransactionKey` carries no client/server role, so an endpoint that sends
            // a request to itself — a proxy, a B2BUA, a loopback test — can have a live *client*
            // transaction under the same key. Cleaning the shared maps then closes that
            // client's response stream and strands its retransmissions, which is a worse fault
            // than the leak being fixed.
            if self.clients.contains_key(&key) {
                continue;
            }
            self.timers.forget_matching(|(k, _)| k == &key);
            self.destinations.remove(&key);
            // `reconnect` too. It is removed nowhere else but `Output::Terminated`, which an
            // abandoned transaction never reaches — so leaving it here would trade one
            // unbounded map for another.
            self.reconnect.remove(&key);
        }
    }

    async fn on_timers(&mut self) {
        let due = self.timers.take_due(tokio::time::Instant::now());
        for (key, timer) in due {
            let outputs = self.layer.on_timer(&key, timer);
            self.perform(&key, outputs, None).await;
        }
    }

    async fn on_command(&mut self, command: Command) {
        match command {
            Command::Request {
                request,
                target,
                events,
                reply,
            } => {
                let Some((key, outputs)) = self
                    .layer
                    .send_request(*request, target.transport.reliability())
                else {
                    let _ = reply.send(Err(Error::NoVia));
                    return;
                };
                self.destinations.insert(key.clone(), target);
                self.clients.insert(key.clone(), events);
                let _ = reply.send(Ok(key.clone()));
                self.perform(&key, outputs, None).await;
            }
            Command::Respond {
                key,
                response,
                sent,
            } => {
                // Nothing is removed from `handed_over` here, and that is the point. A
                // provisional response is not an answer: an application that sends 180 Ringing
                // and then wedges has a transaction sitting in `Proceeding`, which RFC 3261
                // §17.2.1 gives no timer either. Clearing on any response would exempt exactly
                // the calls most likely to be abandoned — the ones that rang. A transaction
                // that *is* answered reaches `Output::Terminated` through Timer J at 32 s, well
                // inside the limit, and is cleaned there.
                if self.layer.server_request(&key).is_none() {
                    // No transaction to answer on. Reporting success here would tell an
                    // application its 200 OK went out while the caller heard nothing — the
                    // caller times out believing the call failed, the callee believes it is up.
                    let _ = sent.send(Err(Error::NoTransaction));
                    return;
                }
                let outputs = self.layer.send_response(&key, *response);
                // The success reported here is *produced by* the send, not written after it: the
                // only way to obtain the `Ok` is from the `Performed` that `perform` hands back.
                // Reversing these two statements does not compile, which is the guarantee `X-36`
                // asked for — a test could not see the difference, because on a `current_thread`
                // runtime the oneshot does not yield and `perform` finished either way.
                let performed = self.perform(&key, outputs, None).await;
                let _ = sent.send(performed.into_result());
            }
            Command::Direct {
                request,
                target,
                sent,
            } => {
                let bytes = Message::Request(*request).to_bytes();
                let result = self.transmit(bytes, target, false, None).await;
                let _ = sent.send(result);
            }
            Command::WatchUnmatched(sink) => {
                // Replaces rather than fans out. Two watchers would each see some of the
                // responses and neither would see all of them, which is worse than one watcher
                // and much worse than an error.
                self.unmatched = Some(sink);
            }
            Command::Keepalive { target, answered } => {
                self.on_keepalive(target, answered).await;
            }
            Command::Outstanding(reply) => {
                let (clients, servers) = self.layer.len();
                // Every per-transaction map, not just the transactions. An entry that outlives
                // its transaction is exactly the leak a count of transactions alone would miss,
                // and a map left out here is a map a soak run is structurally blind to.
                let _ = reply.send(
                    clients
                        + servers
                        + self.destinations.len()
                        + self.reconnect.len()
                        + self.handed_over.len(),
                );
            }
            Command::Shutdown => {}
        }
    }

    /// Perform a transaction's outputs, in order.
    async fn perform(
        &mut self,
        key: &TransactionKey,
        outputs: Vec<Output>,
        origin: Option<(SocketAddr, TransportKind)>,
    ) -> Performed {
        for output in outputs {
            match output {
                Output::Send(message) => {
                    let target =
                        self.destinations.get(key).cloned().or_else(|| {
                            origin.map(|(addr, transport)| Target::new(addr, transport))
                        });
                    let Some(target) = target else {
                        tracing::warn!("no destination for a message the transaction wants sent");
                        continue;
                    };
                    let is_response = matches!(*message, Message::Response(_));
                    let bytes = message.to_bytes();
                    let addr = target.addr;
                    let fallback = self.reconnect.get(key).cloned();
                    if let Err(error) = self.transmit(bytes, target, is_response, fallback).await {
                        tracing::warn!(%error, %addr, "send failed");
                        let outputs = self.layer.on_transport_error(key);
                        return Box::pin(self.perform(key, outputs, origin)).await;
                    }
                }
                // The clock is read *here*, by the driver, and handed to the queue. That is what
                // lets any other driver — one on virtual time, say — use the same queue.
                Output::SetTimer { timer, after } => {
                    self.timers
                        .set((key.clone(), timer), tokio::time::Instant::now(), after);
                }
                Output::ClearTimer(timer) => self.timers.clear(&(key.clone(), timer)),
                Output::ToTu(event) => self.deliver(key, *event, origin).await,
                Output::Terminated(_) => {
                    self.timers.forget_matching(|(k, _)| k == key);
                    self.destinations.remove(key);
                    self.handed_over.remove(key);
                    self.reconnect.remove(key);
                    // Dropping the sender closes the application's response stream, which is
                    // how it learns the transaction is over.
                    self.clients.remove(key);
                }
            }
        }
        Performed
    }

    /// Put bytes on the wire that are not a SIP message.
    ///
    /// A keep-alive is not a request and must not be treated as one: no MTU refusal (a STUN
    /// header is 20 bytes), no transaction, no `Via`. It reuses [`Driver::transmit`] so a flow's
    /// ping travels over the *same connection* its requests do — which is the whole of RFC 5626
    /// §4.4, since a ping on a second connection tests a flow nobody is using.
    async fn transmit_raw(&mut self, bytes: Bytes, target: &Target) -> Result<()> {
        self.transmit(bytes, target.clone(), true, None).await
    }

    /// Put bytes on the wire, opening a connection if the transport needs one.
    ///
    /// `is_response` decides whether an inbound connection may be used. A response goes back
    /// over the connection its request arrived on — RFC 5923, and the only thing that works
    /// when the peer is behind a NAT. An outbound *request* is different: reusing an inbound
    /// connection for one is how a peer that connected to you gets your traffic routed
    /// through it, so that is off unless configured.
    async fn transmit(
        &mut self,
        bytes: Bytes,
        target: Target,
        is_response: bool,
        fallback: Option<Target>,
    ) -> Result<()> {
        match target.transport {
            TransportKind::Udp => {
                // RFC 3261 §18.1.1. Refusing by name beats sending something that will be
                // fragmented or silently truncated — a truncated SIP message is a security
                // problem, not a degraded one.
                //
                // Requests only. §18.1.1 offers a sender the alternative of switching to a
                // congestion-controlled transport; §18.2.2 offers a *responder* nothing — the
                // response goes back per the topmost `Via`, over the transport the request
                // came in on. Refusing it here would answer a 200 with silence, leaving the
                // caller to time out while the callee believes the call is up.
                if !is_response && bytes.len() > self.mtu {
                    return Err(Error::TooLarge {
                        size: bytes.len(),
                        mtu: self.mtu,
                    });
                }
                self.socket.send_to(&bytes, target.addr).await?;
                Ok(())
            }
            TransportKind::Tcp => {
                let key = target.connection();
                if is_response && self.pool.send_on_existing(&key, bytes.clone()).await {
                    return Ok(());
                }
                // The connection is gone. RFC 3261 §18.2.2 sends the response to the address
                // the request came from at the port the sender said it listens on — not back
                // at the ephemeral port it dialled out from, where nothing is accepting.
                let key = match (is_response, &fallback) {
                    (true, Some(advertised)) => advertised.connection(),
                    _ => key,
                };
                self.pool.send(&key, bytes).await
            }
            #[cfg(feature = "tls")]
            TransportKind::Tls => {
                // Answering on the connection the request arrived over comes first, and needs
                // no client configuration at all — a pure TLS server has no reason to hold
                // one, and requiring it would leave such a server unable to reply.
                let key = target.connection();
                if is_response && self.pool.send_on_existing(&key, bytes.clone()).await {
                    return Ok(());
                }
                // Only opening a *new* connection needs somewhere to verify against.
                let Some(client) = self.tls_client.clone() else {
                    return Err(Error::UnsupportedTransport(
                        "TLS (no client configuration, so no outbound connection can be verified)",
                    ));
                };
                // The name a certificate is checked against is the host from the URI, carried
                // on the target rather than derived from the address it resolved to.
                let name = target
                    .verify_as
                    .as_deref()
                    .map_or_else(|| target.addr.ip().to_string(), str::to_owned);
                self.pool.send_tls(&key, &name, &client, bytes).await
            }
            #[cfg(feature = "ws")]
            TransportKind::Ws | TransportKind::Wss => {
                let key = target.connection();
                // Unconditionally, and not only for responses. A WebSocket peer has no
                // listening port (RFC 7118 §5.2), so an existing connection is not merely the
                // preferred way to reach it — it is the only one. The pool's "do not carry
                // outbound requests over an inbound connection" rule protects against traffic
                // being routed through a peer that connected to us; here the peer *is* the
                // destination, so there is nothing to route through and nothing to protect.
                if self.pool.send_on_existing(&key, bytes.clone()).await {
                    return Ok(());
                }
                let authority = target.verify_as.as_deref().map_or_else(
                    || target.addr.to_string(),
                    |name| format!("{name}:{}", target.addr.port()),
                );
                self.pool
                    .send_ws(
                        &key,
                        &authority,
                        self.ws_keepalive,
                        #[cfg(feature = "wss")]
                        self.tls_client.as_ref(),
                        bytes,
                    )
                    .await
            }
            #[allow(unreachable_patterns)]
            other => Err(Error::UnsupportedTransport(other.as_str())),
        }
    }

    async fn deliver(
        &mut self,
        key: &TransactionKey,
        event: TuEvent,
        origin: Option<(SocketAddr, TransportKind)>,
    ) {
        // A client transaction's events go to whoever sent the request.
        if let Some(sender) = self.clients.get(key) {
            let _ = sender.send(event).await;
            return;
        }

        let (source, transport) = origin.unwrap_or((self.local_addr(), TransportKind::Udp));
        match event {
            TuEvent::Request(request) | TuEvent::Ack(request) => {
                let is_ack = request.method == sipx_sip::Method::Ack;
                if self
                    .incoming
                    .try_send(Incoming {
                        key: key.clone(),
                        request: *request,
                        source,
                        transport,
                    })
                    .is_err()
                {
                    // The application is not keeping up. Blocking the loop would stop timers,
                    // which turns a slow application into a stack that drops established
                    // calls; dropping the event silently loses a request.
                    if is_ack {
                        // An ACK cannot be refused. SIP has no response to an ACK, and an ACK
                        // for a 2xx is a transaction of its own (RFC 3261 §17.1.1.3) with
                        // nothing to answer — so there is no 503 to send, nothing will
                        // retransmit it once Timer H expires, and both ends are left in a
                        // dialog no timer reaps unless RFC 4028 session timers happen to be
                        // running. This is the one that leaks calls, which is why it is
                        // counted apart and logged at error rather than warn.
                        self.shed.acks.fetch_add(1, Ordering::Relaxed);
                        tracing::error!(
                            %source,
                            "application queue full; an ACK was dropped and cannot be refused — \
                             the dialog it would have completed will not be reaped"
                        );
                    } else {
                        self.shed.requests.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(%source, "application queue full; refusing the transaction");
                        self.refuse(key).await;
                    }
                }
            }
            _ => {}
        }
    }

    async fn refuse(&mut self, key: &TransactionKey) {
        let Some(status) = sipx_sip::StatusCode::new(503) else {
            return;
        };
        let Some(request) = self.layer.server_request(key).cloned() else {
            return;
        };
        let Ok(builder) =
            sipx_sip::build::ResponseBuilder::to_request(&request, status, "Service Unavailable")
        else {
            return;
        };
        let Ok(builder) = builder.header(HeaderName::RetryAfter, Bytes::from_static(b"5")) else {
            return;
        };
        let outputs = self.layer.send_response(key, builder.build());
        Box::pin(self.perform(key, outputs, None)).await;
    }

    fn local_addr(&self) -> SocketAddr {
        self.socket
            .local_addr()
            .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)))
    }
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        // Never resolves; the `if` guard in `select!` keeps this branch disabled anyway.
        None => std::future::pending().await,
    }
}
