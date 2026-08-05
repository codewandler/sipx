//! The endpoint: one event loop driving the sans-IO core.
//!
//! Everything mutable lives in this loop — the transaction layer, the timer queue, the
//! sockets. No transaction is reachable from two tasks, so there are no locks in the
//! signalling path and no way to observe a half-applied transition. Applications talk to the
//! loop over channels.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;
use sipx_sip::transaction::{Dispatch, Output, Timer, TransactionKey, TransactionLayer, TuEvent};
use sipx_sip::{Header, HeaderName, Limits, Message, Request, Response, Timers, parse_datagram};
use tokio::net::{TcpListener, UdpSocket};
#[cfg(any(feature = "tls", feature = "ws"))]
use tokio::sync::Semaphore;
#[cfg(feature = "tls")]
use tokio::sync::watch;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use crate::capture::{Capture, CaptureConfig, Direction};
use crate::counters::{Counters, Meters, ShedCounts};
use crate::error::{Error, Result};
use crate::nat::apply_received_and_rport;
use crate::overload::{Controller as OverloadController, OverloadConfig};
use crate::policy::{
    ConnectionState, EndpointObservation, MessageDirection, MessageObservation, ObservationHub,
    RequestPolicyDecision, RequestPolicyRef, SourceAdmission, SourcePrefix, TransactionClass,
    connection_event, duplicate_policy_header, policy_header,
};
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
    /// Most incomplete inbound TLS, WebSocket and secure-WebSocket handshakes at once.
    pub handshake_limit: usize,
    /// How long an inbound handshake may remain incomplete.
    pub handshake_timeout: std::time::Duration,
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
    /// How sipx verifies an outbound QUIC peer.
    #[cfg(feature = "quic")]
    pub quic_client: Option<crate::tls::ClientTls>,
    /// The identity and UDP port for the experimental QUIC listener.
    #[cfg(feature = "quic")]
    pub quic_server: Option<(crate::tls::ServerTls, u16)>,
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
    /// Record the signalling this endpoint exchanges to a file (§13).
    ///
    /// `None` — the default — costs one `Option` check per message and opens nothing. **A capture
    /// contains call content and identities even after redaction**; see [`CaptureConfig`].
    pub capture: Option<CaptureConfig>,
    /// Hop-by-hop overload feedback, client advertisement, rate tolerance, prioritization, and
    /// randomness. Client advertisement is off by default; see [`OverloadConfig::advertise`].
    pub overload: OverloadConfig,
    /// Optional immutable pre-transaction request policy.
    pub request_policy: Option<RequestPolicyRef>,
    /// Maximum number of IP/CIDR entries in one live source-admission generation.
    ///
    /// Every admission check is a linear scan, so this is a work bound as well as a memory bound.
    pub source_admission_limit: usize,
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
            handshake_limit: 64,
            handshake_timeout: std::time::Duration::from_secs(10),
            mtu: 1300,
            tcp: true,
            #[cfg(feature = "tls")]
            tls_client: None,
            #[cfg(feature = "tls")]
            tls_server: None,
            #[cfg(feature = "quic")]
            quic_client: None,
            #[cfg(feature = "quic")]
            quic_server: None,
            #[cfg(feature = "ws")]
            ws_server: None,
            #[cfg(feature = "wss")]
            wss_server: None,
            capture: None,
            overload: OverloadConfig::default(),
            request_policy: None,
            source_admission_limit: 1024,
            #[cfg(feature = "ws")]
            ws_keepalive: std::time::Duration::from_secs(25),
            unanswered_limit: std::time::Duration::from_secs(180),
            pool: PoolConfig::default(),
        }
    }

    fn validate(&self) -> Result<()> {
        let nonzero = |field| Error::InvalidConfig {
            field,
            reason: "must be non-zero",
        };
        if self.capacity == 0 {
            return Err(nonzero("capacity"));
        }
        if self.pool.max_connections == 0 {
            return Err(nonzero("pool.max_connections"));
        }
        if self.handshake_limit == 0 {
            return Err(nonzero("handshake_limit"));
        }
        if self.handshake_timeout.is_zero() {
            return Err(nonzero("handshake_timeout"));
        }
        if self.source_admission_limit == 0 {
            return Err(nonzero("source_admission_limit"));
        }
        if self.overload.validity.is_zero() {
            return Err(nonzero("overload.validity"));
        }
        if self.overload.validity.as_millis() == 0 {
            return Err(Error::InvalidConfig {
                field: "overload.validity",
                reason: "must be at least one millisecond",
            });
        }
        if self.overload.peer_limit == 0 {
            return Err(nonzero("overload.peer_limit"));
        }
        if matches!(self.overload.feedback, crate::OverloadFeedback::Loss(value) if value > 100) {
            return Err(Error::InvalidConfig {
                field: "overload.feedback",
                reason: "loss percentage must be between 0 and 100",
            });
        }
        if self.overload.rate_tolerance_intervals >= self.overload.rate_priority_tolerance_intervals
        {
            return Err(Error::InvalidConfig {
                field: "overload.rate_priority_tolerance_intervals",
                reason: "must be greater than overload.rate_tolerance_intervals",
            });
        }
        #[cfg(feature = "ws")]
        if self.ws_keepalive.is_zero() {
            return Err(nonzero("ws_keepalive"));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Background {
    cancel: CancellationToken,
    tasks: TaskTracker,
    owns_lifetime: bool,
}

#[derive(Debug, Default)]
struct ShutdownState {
    complete: AtomicBool,
    notify: tokio::sync::Notify,
}

impl ShutdownState {
    async fn wait(&self) {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.complete.load(Ordering::SeqCst) {
            notified.await;
        }
    }

    fn complete(&self) {
        self.complete.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

impl Clone for Background {
    fn clone(&self) -> Self {
        Self {
            cancel: self.cancel.clone(),
            tasks: self.tasks.clone(),
            owns_lifetime: false,
        }
    }
}

impl Background {
    fn new() -> Self {
        Self {
            cancel: CancellationToken::new(),
            tasks: TaskTracker::new(),
            owns_lifetime: true,
        }
    }

    fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.tasks.spawn(future);
    }

    async fn shutdown(&self) {
        self.cancel.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        if self.owns_lifetime {
            // This also covers a later bind failing after an earlier optional listener started.
            // Cloned task handles do not own the lifetime and therefore cannot cancel siblings.
            self.cancel.cancel();
            self.tasks.close();
        }
    }
}

fn apply_network_source(message: Message, source: SocketAddr) -> Message {
    match message {
        Message::Request(mut request) => {
            apply_received_and_rport(&mut request, source);
            Message::Request(request)
        }
        response @ Message::Response(_) => response,
    }
}

#[cfg(any(feature = "tls", feature = "ws"))]
#[derive(Debug, Clone)]
struct HandshakeRuntime {
    deadline: std::time::Duration,
    permits: Arc<Semaphore>,
    owner: Background,
    #[cfg(test)]
    observations: Option<mpsc::UnboundedSender<HandshakeObservation>>,
}

/// The configured identity plus the endpoint-wide replacement selected by later handshakes.
///
/// Kept as one argument so TLS and WSS cannot accidentally read the publication channel and then
/// construct an acceptor from a different configured policy.
#[cfg(feature = "tls")]
#[derive(Debug, Clone)]
struct ServerHandshakePolicy {
    configured: crate::tls::ServerTls,
    replacement: watch::Receiver<Option<crate::tls::ServerTls>>,
}

#[cfg(feature = "tls")]
impl ServerHandshakePolicy {
    fn new(
        configured: crate::tls::ServerTls,
        replacement: watch::Receiver<Option<crate::tls::ServerTls>>,
    ) -> Self {
        Self {
            configured,
            replacement,
        }
    }

    fn acceptor(&self) -> tokio_rustls::TlsAcceptor {
        // One immutable configuration is selected by one watch-channel read. A concurrent reload
        // may leave this handshake old or make it new, never split certificate chain from key.
        self.replacement
            .borrow()
            .as_ref()
            .unwrap_or(&self.configured)
            .acceptor()
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandshakeObservation {
    Admitted,
    Refused,
}

#[cfg(test)]
fn observe_handshake(
    observations: Option<&mpsc::UnboundedSender<HandshakeObservation>>,
    observation: HandshakeObservation,
) {
    if let Some(observations) = observations {
        // discard: observations exist only as a unit-test barrier and the test may already have
        // ended while endpoint cleanup is still unwinding.
        let _ = observations.send(observation);
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
    failures: mpsc::Receiver<Error>,
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

    /// Take the concrete driver failure associated with a `TransportError` event.
    ///
    /// The sans-I/O transaction layer carries only the fact that transport failed. The endpoint
    /// queues the I/O-layer cause before feeding that fact into the core, which lets an application
    /// preserve a TLS verification error instead of reporting an unanswered request.
    pub fn take_transport_error(&mut self) -> Option<Error> {
        self.failures.try_recv().ok()
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
        failures: mpsc::Sender<Error>,
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
    /// Stop the driver after every listener, handshake and pooled connection has terminated.
    Shutdown,
}

#[derive(Debug)]
struct ClientSink {
    events: mpsc::Sender<TuEvent>,
    failures: mpsc::Sender<Error>,
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

/// A handle to a running endpoint.
#[derive(Debug, Clone)]
pub struct Handle {
    commands: mpsc::Sender<Command>,
    shutdown: Arc<ShutdownState>,
    local_addr: SocketAddr,
    /// Every counter, shared with the driver so they can be read while the driver is busy —
    /// which is the only time they are interesting (§12).
    meters: Arc<Meters>,
    admission: Arc<SourceAdmission>,
    observations: Arc<ObservationHub>,
    request_policy: Option<RequestPolicyRef>,
    #[cfg(feature = "tls")]
    tls_addr: Option<SocketAddr>,
    /// Atomic publication point for the identity selected by later TLS and WSS handshakes.
    ///
    /// `None` when neither listener exists. QUIC deliberately does not subscribe: its live
    /// configuration and connection lifetime are a separate contract (`sip-tls.md` §3.6).
    #[cfg(feature = "tls")]
    server_identity: Option<watch::Sender<Option<crate::tls::ServerTls>>>,
    #[cfg(feature = "ws")]
    ws_addr: Option<SocketAddr>,
    #[cfg(feature = "wss")]
    wss_addr: Option<SocketAddr>,
    #[cfg(feature = "quic")]
    quic_addr: Option<SocketAddr>,
    /// The sent-by this endpoint uses on a WebSocket it dialled out (RFC 7118 §5.2).
    ///
    /// Invented once at bind time rather than per request: a `Via` that changed between a
    /// request and its retransmission would be a different `Via`.
    #[cfg(feature = "ws")]
    ws_sent_by: Arc<str>,
    advertise_overload: bool,
    sent_by: Arc<String>,
    sent_by_port: u16,
}

impl Handle {
    /// Replace the complete live source-admission set and return its generation.
    ///
    /// An empty set refuses every new source. Use [`Self::clear_source_admission`] to allow all.
    ///
    /// # Errors
    ///
    /// Returns [`Error::SourceAdmissionCapacity`] without changing the active generation when
    /// `prefixes` exceeds [`Config::source_admission_limit`].
    pub fn replace_source_admission(&self, prefixes: Vec<SourcePrefix>) -> Result<u64> {
        self.admission.replace(prefixes)
    }

    /// Clear source admission to allow all new sources and return the new generation.
    pub fn clear_source_admission(&self) -> u64 {
        self.admission.clear()
    }

    /// Replace the optional bounded endpoint observer.
    ///
    /// Producers never await this receiver. A full receiver drops and increments
    /// [`Counters::observation_dropped`]; dropping it simply detaches observation.
    #[must_use]
    pub fn observe(&self, capacity: usize) -> mpsc::Receiver<EndpointObservation> {
        self.observations.subscribe(capacity)
    }

    fn apply_request_policy(&self, request: &mut Request, target: &Target) -> Result<()> {
        let Some(policy) = &self.request_policy else {
            return Ok(());
        };
        match policy.decide(request, target) {
            RequestPolicyDecision::Allow => Ok(()),
            RequestPolicyDecision::Reject(reason) => Err(Error::PolicyRejected { reason }),
            RequestPolicyDecision::AddHeaders(headers) => {
                for header in headers {
                    let (semantic, allowed) = policy_header(header.name());
                    if !allowed || duplicate_policy_header(request, &semantic) {
                        return Err(Error::ProtectedPolicyHeader {
                            name: String::from_utf8_lossy(semantic.canonical()).into_owned(),
                        });
                    }
                    request.headers.push(header);
                }
                Ok(())
            }
        }
    }

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

    /// Replace the identity selected by new TLS and WSS server handshakes (§3.6).
    ///
    /// Validation happens before publication: the complete certificate chain and private key are
    /// first turned into one immutable [`crate::tls::ServerTls`] configuration. If they do not
    /// belong together, this returns a typed TLS error and the active configuration is untouched.
    ///
    /// Existing connections are not renegotiated or closed. File watching and secret-store I/O
    /// belong to the host, which supplies an already parsed [`crate::tls::Identity`] here.
    #[cfg(feature = "tls")]
    pub fn reload_server_identity(&self, identity: crate::tls::Identity) -> Result<()> {
        let Some(publication) = &self.server_identity else {
            return Err(Error::InvalidConfig {
                field: "server_identity",
                reason: "reload requires a configured TLS or WSS server listener",
            });
        };
        let replacement = crate::tls::ServerTls::new(identity).map_err(|error| {
            tracing::warn!(%error, "TLS server identity reload refused");
            Error::Tls(error)
        })?;
        publication.send(Some(replacement)).map_err(|_| {
            tracing::warn!("TLS server identity reload refused because no secure listener remains");
            Error::InvalidConfig {
                field: "server_identity",
                reason: "no TLS or WSS server listener is running",
            }
        })?;
        tracing::info!("TLS server identity reloaded for new TLS and WSS handshakes");
        Ok(())
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
        self.apply_request_policy(&mut request, &target)?;
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
        if self.advertise_overload {
            crate::overload::advertise(&mut request);
        }

        let (events_tx, events_rx) = mpsc::channel(32);
        let (failures_tx, failures_rx) = mpsc::channel(1);
        let (reply_tx, reply_rx) = oneshot::channel();
        self.commands
            .send(Command::Request {
                request: Box::new(request),
                target,
                events: events_tx,
                failures: failures_tx,
                reply: reply_tx,
            })
            .await
            .map_err(|_| Error::EndpointClosed)?;
        reply_rx.await.map_err(|_| Error::EndpointClosed)??;
        Ok(Responses {
            rx: events_rx,
            failures: failures_rx,
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
    pub async fn send_directly(&self, mut request: Request, target: Target) -> Result<()> {
        self.apply_request_policy(&mut request, &target)?;
        if self.advertise_overload {
            crate::overload::advertise(&mut request);
        }
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
        #[cfg(feature = "quic")]
        if transport == TransportKind::Quic
            && let Some(addr) = self.quic_addr
        {
            return format!("{}:{}", self.sent_by, addr.port());
        }
        // discard: not a loss. The parameter is unused unless a transport feature is on, and
        // this is the suppressor rather than a discarded result.
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
        self.meters.snapshot().shed
    }

    /// Everything this endpoint will say about itself (§12).
    ///
    /// Synchronous, and deliberately so. [`Self::outstanding`] beside it is `async` and returns a
    /// `Result` because it asks the event loop; this reads shared atomics and cannot fail, because a
    /// snapshot that was unavailable while the loop was busy would be unavailable in exactly the
    /// situation an operator reaches for it.
    ///
    /// A snapshot is **not a consistent instant** — see [`Counters`] for what that does and does not
    /// allow you to conclude.
    #[must_use]
    pub fn counters(&self) -> Counters {
        self.meters.snapshot()
    }

    /// Address of the experimental QUIC listener, when configured.
    #[cfg(feature = "quic")]
    #[must_use]
    pub fn quic_addr(&self) -> Option<SocketAddr> {
        self.quic_addr
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
        if !self.shutdown.complete.load(Ordering::SeqCst) {
            // discard: a closed command channel means shutdown has already begun. The shared
            // durable barrier below still waits for cleanup, including for callers arriving late.
            let _ = self.commands.send(Command::Shutdown).await;
            self.shutdown.wait().await;
        }
    }
}

fn branch_with_rng<R>(rng: &mut R) -> String
where
    R: rand::CryptoRng + ?Sized,
{
    let value = rand::RngCore::next_u64(rng);
    format!("z9hG4bK{value:016x}")
}

/// A `branch` token: the RFC's magic cookie plus 64 bits from a cryptographic RNG.
///
/// The width is ours, not the RFC's. A guessable branch lets an off-path attacker inject
/// responses into a transaction, so this is not a place for a counter.
#[must_use]
pub fn new_branch() -> String {
    branch_with_rng(&mut rand::rng())
}

/// Bind an endpoint and start its loop.
///
/// Returns a handle for sending, and a receiver of the requests that arrive.
#[allow(
    clippy::too_many_lines,
    reason = "one ordered assembly keeps validation, every bind and task ownership auditable"
)]
pub async fn bind(config: Config) -> Result<(Handle, mpsc::Receiver<Incoming>)> {
    config.validate()?;
    let (socket, listener, local_addr) = bind_matching_ports(&config).await?;
    let background = Background::new();
    let meters = Arc::new(Meters::default());
    let admission = Arc::new(SourceAdmission::new(config.source_admission_limit));
    let observations = Arc::new(ObservationHub::new(Arc::clone(&meters)));
    #[cfg(any(feature = "tls", feature = "ws"))]
    let handshakes = HandshakeRuntime {
        deadline: config.handshake_timeout,
        permits: Arc::new(Semaphore::new(config.handshake_limit)),
        owner: background.clone(),
        #[cfg(test)]
        observations: None,
    };
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

    // A single publication point feeds both secure stream listeners. Their configured identities
    // may differ before the first reload; afterwards both select the one complete replacement.
    // QUIC does not receive this channel (`sip-tls.md` §3.6).
    #[cfg(feature = "tls")]
    let (server_identity_tx, server_identity_rx) =
        watch::channel::<Option<crate::tls::ServerTls>>(None);
    #[cfg(feature = "tls")]
    let has_reloadable_server = config.tls_server.is_some() || {
        #[cfg(feature = "wss")]
        {
            config.wss_server.is_some()
        }
        #[cfg(not(feature = "wss"))]
        {
            false
        }
    };

    #[cfg(feature = "tls")]
    let secure_addr = match config.tls_server.clone() {
        Some((server, port)) => Some(
            listen_tls(
                config.bind.ip(),
                port,
                ServerHandshakePolicy::new(server, server_identity_rx.clone()),
                &adopt_tx,
                &handshakes,
                Arc::clone(&admission),
                Arc::clone(&meters),
            )
            .await?,
        ),
        None => None,
    };
    #[cfg(feature = "ws")]
    let upgrade_addr = match config.ws_server {
        Some(port) => Some(
            listen_ws(
                config.bind.ip(),
                port,
                config.ws_keepalive,
                config.limits,
                &adopt_tx,
                &handshakes,
                Arc::clone(&admission),
                Arc::clone(&meters),
            )
            .await?,
        ),
        None => None,
    };
    #[cfg(feature = "wss")]
    let secure_upgrade_addr = match config.wss_server.clone() {
        Some((server, port)) => Some(
            listen_wss(
                config.bind.ip(),
                port,
                ServerHandshakePolicy::new(server, server_identity_rx.clone()),
                config.ws_keepalive,
                config.limits,
                &adopt_tx,
                &handshakes,
                Arc::clone(&admission),
                Arc::clone(&meters),
            )
            .await?,
        ),
        None => None,
    };
    #[cfg(feature = "quic")]
    let quic_endpoint = if config.quic_client.is_some() || config.quic_server.is_some() {
        let port = config.quic_server.as_ref().map_or(0, |(_, port)| *port);
        Some(crate::quic::endpoint(
            config.bind.ip(),
            port,
            config.quic_client.as_ref(),
            config.quic_server.as_ref().map(|(server, _)| server),
        )?)
    } else {
        None
    };
    #[cfg(feature = "quic")]
    let quic_addr = match (&quic_endpoint, &config.quic_server) {
        (Some(endpoint), Some(_)) => {
            let addr = endpoint.local_addr()?;
            listen_quic(
                endpoint.clone(),
                &adopt_tx,
                &handshakes,
                Arc::clone(&admission),
                Arc::clone(&meters),
            );
            Some(addr)
        }
        _ => None,
    };

    let (commands_tx, commands_rx) = mpsc::channel(config.capacity);
    let (incoming_tx, incoming_rx) = mpsc::channel(config.capacity);
    let shutdown = Arc::new(ShutdownState::default());

    let handle = Handle {
        commands: commands_tx,
        shutdown: Arc::clone(&shutdown),
        local_addr,
        meters: Arc::clone(&meters),
        admission: Arc::clone(&admission),
        observations: Arc::clone(&observations),
        request_policy: config.request_policy.clone(),
        #[cfg(feature = "tls")]
        tls_addr: secure_addr,
        #[cfg(feature = "tls")]
        server_identity: has_reloadable_server.then_some(server_identity_tx),
        #[cfg(feature = "ws")]
        ws_addr: upgrade_addr,
        #[cfg(feature = "wss")]
        wss_addr: secure_upgrade_addr,
        #[cfg(feature = "quic")]
        quic_addr,
        #[cfg(feature = "ws")]
        ws_sent_by: Arc::from(crate::ws::invented_sent_by()),
        advertise_overload: config.overload.advertise,
        sent_by: Arc::new(config.sent_by.clone()),
        sent_by_port,
    };

    // Started before the driver, so a path that cannot be opened fails `bind` rather than leaving a
    // running endpoint that appears to be recording and writes nothing.
    let capture = match &config.capture {
        Some(wanted) => Some(
            Capture::start(wanted, Arc::clone(&meters)).map_err(|source| Error::Capture {
                path: wanted.path.display().to_string(),
                source,
            })?,
        ),
        None => None,
    };

    let (net_tx, net_rx) = mpsc::channel(config.capacity);
    let (accept_tx, accept_rx) = mpsc::channel(64);
    if let Some(listener) = listener {
        let cancel = background.cancel.clone();
        background.spawn(accept_tcp_until(
            listener,
            accept_tx,
            cancel,
            Arc::clone(&admission),
            Arc::clone(&meters),
        ));
    }

    let driver = Driver {
        socket: Arc::new(socket),
        layer: TransactionLayer::new(config.timers),
        timers: TimerQueue::new(),
        destinations: HashMap::new(),
        transaction_generations: HashMap::new(),
        handed_over: HashMap::new(),
        reconnect: HashMap::new(),
        unanswered_limit: config.unanswered_limit,
        overload: OverloadController::new(
            config.overload.rate_tolerance_intervals,
            config.overload.rate_priority_tolerance_intervals,
            config.overload.peer_limit,
        ),
        overload_config: config.overload.clone(),
        overload_epoch: tokio::time::Instant::now(),
        overload_sequence: 0,
        server_overloaded_until: None,
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
        #[cfg(feature = "quic")]
        quic_client: config.quic_client.clone(),
        #[cfg(feature = "quic")]
        quic_endpoint,
        pool: Pool::new_observed(
            config.pool,
            config.limits,
            net_tx,
            Arc::clone(&observations),
        ),
        limits: config.limits,
        mtu: config.mtu,
        meters,
        admission,
        observations,
        capture,
        local_addr,
        unmatched: None,
        stun_waiters: HashMap::new(),
        pong_waiters: HashMap::new(),
        #[cfg(feature = "quic")]
        quic_replies: HashMap::new(),
        background,
        shutdown,
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
    server: ServerHandshakePolicy,
    adopt: &mpsc::Sender<Adopt>,
    runtime: &HandshakeRuntime,
    admission: Arc<SourceAdmission>,
    meters: Arc<Meters>,
) -> Result<SocketAddr> {
    let listener = TcpListener::bind(SocketAddr::new(ip, port)).await?;
    let addr = listener.local_addr()?;
    let adopt = adopt.clone();
    let owner = runtime.owner.clone();
    let cancel = owner.cancel.clone();
    let permits = Arc::clone(&runtime.permits);
    let deadline = runtime.deadline;
    #[cfg(test)]
    let observations = runtime.observations.clone();
    runtime.owner.spawn(async move {
        loop {
            let accepted = tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                accepted = listener.accept() => accepted,
            };
            let (stream, peer) = match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(%error, "TLS accept failed");
                    break;
                }
            };
            let Some(admission_generation) = admission.admit(peer.ip()) else {
                meters.source_refusal(TransportKind::Tls);
                tracing::debug!(%peer, "refused inbound TLS source before handshake");
                continue;
            };
            let permit = Arc::clone(&permits).try_acquire_owned();
            #[cfg(test)]
            observe_handshake(
                observations.as_ref(),
                if permit.is_ok() {
                    HandshakeObservation::Admitted
                } else {
                    HandshakeObservation::Refused
                },
            );
            let Ok(permit) = permit else {
                // discard: the configured no-queue admission policy closes excess unauthenticated
                // sockets immediately; retaining one here would defeat the handshake bound.
                tracing::debug!(%peer, "refused inbound TLS handshake at capacity");
                continue;
            };
            let acceptor = server.acceptor();
            let adopt = adopt.clone();
            let cancel = cancel.clone();
            owner.spawn(async move {
                let outcome = tokio::select! {
                    biased;
                    () = cancel.cancelled() => None,
                    result = tokio::time::timeout(deadline, acceptor.accept(stream)) => Some(result),
                };
                match outcome {
                    Some(Ok(Ok(tls))) => {
                        // Discarded deliberately, with the reason §12.1 asks for rather than a
                        // counter: a send on this channel fails only when the driver has already
                        // stopped, so the connection has nothing left to be adopted *into*. The
                        // socket closes as it drops, which is the correct outcome and not a loss —
                        // and this runs in a task spawned before the driver exists, so there is no
                        // counter in scope to reach for anyway.
                        // discard: see the reason below.
                        tokio::select! {
                            biased;
                            () = cancel.cancelled() => {}
                            result = adopt.send(Box::new(move |pool: &mut Pool| pool.accept_tls_admitted(tls, peer, admission_generation))) => {
                                let _ = result;
                            }
                        }
                    }
                    Some(Ok(Err(error))) => {
                        tracing::debug!(%error, %peer, "inbound TLS handshake failed");
                    }
                    Some(Err(_)) => tracing::debug!(%peer, "inbound TLS handshake timed out"),
                    None => {}
                }
                drop(permit);
            });
        }
    });
    Ok(addr)
}

/// Accept QUIC handshakes off the driver loop and adopt established connections through the
/// same bounded channel as every other optional transport.
#[cfg(feature = "quic")]
fn listen_quic(
    endpoint: quinn::Endpoint,
    adopt: &mpsc::Sender<Adopt>,
    runtime: &HandshakeRuntime,
    admission: Arc<SourceAdmission>,
    meters: Arc<Meters>,
) {
    let adopt = adopt.clone();
    let owner = runtime.owner.clone();
    let cancel = owner.cancel.clone();
    let permits = Arc::clone(&runtime.permits);
    let deadline = runtime.deadline;
    runtime.owner.spawn(async move {
        loop {
            let incoming = tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                incoming = endpoint.accept() => incoming,
            };
            let Some(incoming) = incoming else {
                break;
            };
            let peer = incoming.remote_address();
            let Some(admission_generation) = admission.admit(peer.ip()) else {
                incoming.refuse();
                meters.source_refusal(TransportKind::Quic);
                tracing::debug!(%peer, "refused inbound QUIC source before handshake");
                continue;
            };
            let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                incoming.refuse();
                tracing::debug!(%peer, "refused inbound QUIC handshake at capacity");
                continue;
            };
            let adopt = adopt.clone();
            let cancel = cancel.clone();
            owner.spawn(async move {
                let connected = tokio::select! {
                    biased;
                    () = cancel.cancelled() => None,
                    result = tokio::time::timeout(deadline, incoming) => Some(result),
                };
                match connected {
                    Some(Ok(Ok(connection))) => {
                        let result = adopt
                            .send(Box::new(move |pool: &mut Pool| {
                                pool.accept_quic_admitted(connection, peer, admission_generation);
                            }))
                            .await;
                        if result.is_err() {
                            tracing::debug!(%peer, "QUIC connection lost its endpoint before adoption");
                        }
                    }
                    Some(Ok(Err(error))) => {
                        tracing::debug!(%error, %peer, "inbound QUIC handshake failed");
                    }
                    Some(Err(_)) => tracing::debug!(%peer, "inbound QUIC handshake timed out"),
                    None => {}
                }
                drop(permit);
            });
        }
    });
}

/// Listen for WebSocket connections, upgrading each off the accept path.
#[cfg(feature = "ws")]
#[allow(clippy::too_many_arguments)]
async fn listen_ws(
    ip: std::net::IpAddr,
    port: u16,
    keepalive: std::time::Duration,
    limits: Limits,
    adopt: &mpsc::Sender<Adopt>,
    runtime: &HandshakeRuntime,
    admission: Arc<SourceAdmission>,
    meters: Arc<Meters>,
) -> Result<SocketAddr> {
    let listener = TcpListener::bind(SocketAddr::new(ip, port)).await?;
    let addr = listener.local_addr()?;
    let adopt = adopt.clone();
    let owner = runtime.owner.clone();
    let cancel = owner.cancel.clone();
    let permits = Arc::clone(&runtime.permits);
    let deadline = runtime.deadline;
    #[cfg(test)]
    let observations = runtime.observations.clone();
    runtime.owner.spawn(async move {
        loop {
            let accepted = tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                accepted = listener.accept() => accepted,
            };
            let (stream, peer) = match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(%error, "WebSocket accept failed");
                    break;
                }
            };
            let Some(admission_generation) = admission.admit(peer.ip()) else {
                meters.source_refusal(TransportKind::Ws);
                tracing::debug!(%peer, "refused inbound WebSocket source before handshake");
                continue;
            };
            let permit = Arc::clone(&permits).try_acquire_owned();
            #[cfg(test)]
            observe_handshake(
                observations.as_ref(),
                if permit.is_ok() {
                    HandshakeObservation::Admitted
                } else {
                    HandshakeObservation::Refused
                },
            );
            let Ok(permit) = permit else {
                // discard: the configured no-queue admission policy closes excess unauthenticated
                // sockets immediately; retaining one here would defeat the handshake bound.
                tracing::debug!(%peer, "refused inbound WebSocket handshake at capacity");
                continue;
            };
            let adopt = adopt.clone();
            let cancel = cancel.clone();
            owner.spawn(async move {
                let upgraded = tokio::select! {
                    biased;
                    () = cancel.cancelled() => None,
                    result = tokio::time::timeout(
                        deadline,
                        crate::ws::accept_with_limits(stream, peer, &limits),
                    ) => Some(result),
                };
                match upgraded {
                    Some(Ok(result)) => {
                        adopt_upgraded(
                            result,
                            peer,
                            TransportKind::Ws,
                            keepalive,
                            admission_generation,
                            &adopt,
                            &cancel,
                        )
                        .await;
                    }
                    Some(Err(_)) => tracing::debug!(%peer, "inbound WebSocket handshake timed out"),
                    None => {}
                }
                drop(permit);
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
#[allow(clippy::too_many_arguments)]
async fn listen_wss(
    ip: std::net::IpAddr,
    port: u16,
    server: ServerHandshakePolicy,
    keepalive: std::time::Duration,
    limits: Limits,
    adopt: &mpsc::Sender<Adopt>,
    runtime: &HandshakeRuntime,
    admission: Arc<SourceAdmission>,
    meters: Arc<Meters>,
) -> Result<SocketAddr> {
    let listener = TcpListener::bind(SocketAddr::new(ip, port)).await?;
    let addr = listener.local_addr()?;
    let adopt = adopt.clone();
    let owner = runtime.owner.clone();
    let cancel = owner.cancel.clone();
    let permits = Arc::clone(&runtime.permits);
    let deadline = runtime.deadline;
    #[cfg(test)]
    let observations = runtime.observations.clone();
    runtime.owner.spawn(async move {
        loop {
            let accepted = tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                accepted = listener.accept() => accepted,
            };
            let (stream, peer) = match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(%error, "WSS accept failed");
                    break;
                }
            };
            let Some(admission_generation) = admission.admit(peer.ip()) else {
                meters.source_refusal(TransportKind::Wss);
                tracing::debug!(%peer, "refused inbound WSS source before handshake");
                continue;
            };
            let permit = Arc::clone(&permits).try_acquire_owned();
            #[cfg(test)]
            observe_handshake(
                observations.as_ref(),
                if permit.is_ok() {
                    HandshakeObservation::Admitted
                } else {
                    HandshakeObservation::Refused
                },
            );
            let Ok(permit) = permit else {
                // discard: the configured no-queue admission policy closes excess unauthenticated
                // sockets immediately; retaining one here would defeat the handshake bound.
                tracing::debug!(%peer, "refused inbound WSS handshake at capacity");
                continue;
            };
            let acceptor = server.acceptor();
            let adopt = adopt.clone();
            let cancel = cancel.clone();
            owner.spawn(async move {
                let upgraded = tokio::select! {
                    biased;
                    () = cancel.cancelled() => None,
                    result = tokio::time::timeout(deadline, async move {
                        let tls = acceptor.accept(stream).await.map_err(|error| error.to_string())?;
                        crate::ws::accept_with_limits(tls, peer, &limits)
                            .await
                            .map_err(|error| error.to_string())
                    }) => Some(result),
                };
                match upgraded {
                    Some(Ok(Ok(socket))) => {
                        adopt_upgraded(
                            Ok(socket),
                            peer,
                            TransportKind::Wss,
                            keepalive,
                            admission_generation,
                            &adopt,
                            &cancel,
                        )
                        .await;
                    }
                    Some(Ok(Err(error))) => {
                        tracing::debug!(%error, %peer, "inbound WSS handshake failed");
                    }
                    Some(Err(_)) => tracing::debug!(%peer, "inbound WSS handshake timed out"),
                    None => {}
                }
                drop(permit);
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
    admission_generation: u64,
    adopt: &mpsc::Sender<Adopt>,
    cancel: &CancellationToken,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    match upgraded {
        Ok(socket) => {
            let key = ConnectionKey::new(peer, transport);
            // Discarded deliberately; see the matching site in `listen_tls` for the reason. A failed
            // send here means the driver has stopped, and a connection with no driver to be adopted
            // into is closed by dropping it.
            // discard: see the reason below.
            tokio::select! {
                biased;
                () = cancel.cancelled() => {}
                result = adopt.send(Box::new(move |pool: &mut Pool| {
                        pool.accept_ws_admitted(socket, key, keepalive, admission_generation);
                    })) => {
                    let _ = result;
                }
            }
        }
        Err(error) => tracing::debug!(%error, %peer, "inbound websocket handshake failed"),
    }
}

/// Accept clear TCP only from the current source-admission generation.
async fn accept_tcp_until(
    listener: TcpListener,
    incoming: mpsc::Sender<(tokio::net::TcpStream, SocketAddr, u64)>,
    cancel: CancellationToken,
    admission: Arc<SourceAdmission>,
    meters: Arc<Meters>,
) {
    loop {
        let accepted = tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let (stream, peer) = match accepted {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "accept failed");
                return;
            }
        };
        let Some(generation) = admission.admit(peer.ip()) else {
            meters.source_refusal(TransportKind::Tcp);
            tracing::debug!(%peer, "refused inbound TCP source before stream parsing");
            continue;
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            result = incoming.send((stream, peer, generation)) => {
                if result.is_err() {
                    return;
                }
            }
        }
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
    /// Exact stream incarnation carrying each transaction; UDP transactions have no entry.
    transaction_generations: HashMap<TransactionKey, ConnectionGeneration>,
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
    /// Per-next-hop RFC 7339/RFC 7415 state, serialized with sends and responses on this loop.
    overload: OverloadController,
    overload_config: OverloadConfig,
    overload_epoch: tokio::time::Instant,
    overload_sequence: u64,
    /// Queue-full detector state advertised on responses until its stated validity expires.
    server_overloaded_until: Option<tokio::time::Instant>,
    clients: HashMap<TransactionKey, ClientSink>,
    incoming: mpsc::Sender<Incoming>,
    commands: mpsc::Receiver<Command>,
    net: mpsc::Receiver<tcp::Event>,
    accepts: mpsc::Receiver<(tokio::net::TcpStream, SocketAddr, u64)>,
    adopts: mpsc::Receiver<Adopt>,
    /// Held only to keep the adoption channel open when no optional listener is configured. A
    /// closed channel would leave that `select!` branch resolving instantly on every pass.
    _adopt: mpsc::Sender<Adopt>,
    #[cfg(feature = "tls")]
    tls_client: Option<crate::tls::ClientTls>,
    #[cfg(feature = "ws")]
    ws_keepalive: std::time::Duration,
    #[cfg(feature = "quic")]
    quic_client: Option<crate::tls::ClientTls>,
    #[cfg(feature = "quic")]
    quic_endpoint: Option<quinn::Endpoint>,
    pool: Pool,
    limits: Limits,
    mtu: usize,
    /// Every counter, shared with every [`Handle`]; see [`Counters`].
    meters: Arc<Meters>,
    admission: Arc<SourceAdmission>,
    observations: Arc<ObservationHub>,
    /// The running capture, if one was configured (§13). `None` is the ordinary case.
    capture: Option<Capture>,
    /// The address this endpoint is bound to.
    ///
    /// Stored rather than asked of the socket. It cannot change after `bind`, and
    /// `UdpSocket::local_addr` is a `getsockname(2)` — which a previous version of this called once
    /// per observed message, capture on or off.
    local_addr: SocketAddr,
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
        ConnectionGeneration,
        std::collections::VecDeque<oneshot::Sender<Result<Option<SocketAddr>>>>,
    >,
    /// Exact response route for each server transaction received on a QUIC stream.
    #[cfg(feature = "quic")]
    quic_replies: HashMap<TransactionKey, crate::quic::Reply>,
    /// Listener and pre-pool handshake tasks owned by this endpoint.
    background: Background,
    /// Durable completion barrier shared with callers that arrive after command closure.
    shutdown: Arc<ShutdownState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConnectionGeneration {
    key: ConnectionKey,
    id: u64,
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
                    Some(Command::Shutdown) | None => break,
                    Some(command) => self.on_command(command).await,
                },
                Some(event) = self.net.recv() => self.on_net_event(event).await,
                Some((stream, peer, generation)) = self.accepts.recv() => {
                    self.pool.accept_admitted(stream, peer, generation);
                },
                Some(adopt) = self.adopts.recv() => adopt(&mut self.pool),
                _ = idle_sweep.tick() => {
                    for closed in self.pool.evict_idle() {
                        tracing::debug!(peer = %closed.peer, "closed an idle connection");
                    }
                    self.abandon_unanswered();
                }
            }
        }
        self.commands.close();
        self.background.shutdown().await;
        self.pool.shutdown().await;
        // The acknowledgement must be the driver's final observable action: dropping `self`
        // first releases the UDP socket, command receiver and every remaining channel owner.
        let shutdown = Arc::clone(&self.shutdown);
        drop(self);
        shutdown.complete();
    }

    async fn on_datagram(&mut self, datagram: Bytes, source: SocketAddr) {
        if self.admission.admit(source.ip()).is_none() {
            self.meters.source_refusal(TransportKind::Udp);
            tracing::debug!(%source, "refused inbound UDP source before parsing");
            return;
        }
        // RFC 5389 §7.3's test, before the SIP parser sees it: a STUN response is not a SIP
        // message and would be dropped as malformed, taking the keep-alive with it.
        if crate::stun::is_stun(&datagram) {
            self.on_stun(&datagram, source);
            return;
        }
        // Captured before parsing, so a malformed datagram is captured malformed: the bytes a
        // peer actually sent are the whole point of the exercise (§13.2).
        self.observe(source, TransportKind::Udp, Direction::In, || {
            datagram.clone()
        });

        match parse_datagram(datagram, &self.limits) {
            Ok(message) => {
                self.on_message(
                    message,
                    source,
                    TransportKind::Udp,
                    None,
                    #[cfg(feature = "quic")]
                    None,
                )
                .await;
            }
            Err(error) => {
                // One malformed packet must not disturb the socket. The alternative is a
                // trivial denial of service.
                //
                // Counted as a parse failure and deliberately not as a request or a response:
                // which it would have been is exactly what could not be determined (§12.2).
                self.meters.parse_failure(TransportKind::Udp);
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
                Ok(_) => {
                    self.stun_waiters.insert(id, answered);
                }
                Err(error) => {
                    // discard: the caller stopped waiting. A dropped receiver means nobody is listening
                    // for this answer, so nothing is lost and there is nothing worth counting.
                    let _ = answered.send(Err(error));
                }
            }
            return;
        }

        // §4.4.1: CRLFCRLF is the ping, and the pong is a lone CRLF the peer's parser is
        // otherwise told to ignore.
        match self
            .transmit_raw(Bytes::from_static(b"\r\n\r\n"), &target)
            .await
        {
            Ok(Some(generation)) => {
                self.pong_waiters
                    .entry(generation)
                    .or_default()
                    .push_back(answered);
            }
            Ok(None) => {
                // A connection-oriented target always reports its pool generation.
                let _ = answered.send(Err(Error::ConnectionClosed));
            }
            Err(error) => {
                // discard: the caller stopped waiting. A dropped receiver means nobody is listening
                // for this answer, so nothing is lost and there is nothing worth counting.
                let _ = answered.send(Err(error));
            }
        }
    }

    /// Answer the waiter a STUN reply belongs to (RFC 5626 §4.4.2).
    fn on_stun(&mut self, datagram: &[u8], source: SocketAddr) {
        let Some(reply) = crate::stun::parse_reply(datagram) else {
            // A Binding *Request*: something on the network is treating this socket as a STUN
            // server. Not ours to answer, and not an error worth raising.
            self.meters.discard_stun_unmatched();
            tracing::debug!(%source, "ignoring a STUN message that is not a reply");
            return;
        };
        let Some(waiter) = self.stun_waiters.remove(&reply.id()) else {
            // An unsolicited or late reply. Dropping it is right: matching it to a *different*
            // keep-alive would report one flow's liveness as another's.
            self.meters.discard_stun_unmatched();
            tracing::debug!(%source, "a STUN reply matched no keep-alive");
            return;
        };
        let answer = match reply {
            crate::stun::Reply::Bound { mapped, .. } => Ok(mapped),
            // §4.4.2: "If a STUN Binding Error Response is received ... the UA considers the flow
            // failed."
            crate::stun::Reply::Failed { .. } => Err(Error::KeepaliveRefused),
        };
        // discard: the caller stopped waiting. A dropped receiver means nobody is listening
        // for this answer, so nothing is lost and there is nothing worth counting.
        let _ = waiter.send(answer);
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive transport-event dispatch keeps ordering and loss accounting visible"
    )]
    async fn on_net_event(&mut self, event: tcp::Event) {
        match event {
            tcp::Event::Message {
                message,
                source,
                transport,
                id,
                #[cfg(feature = "quic")]
                quic_reply,
            } => {
                // Re-serialised rather than raw: framing happened in the connection's task and the
                // stream bytes are not retained, so §13.2 records that a stream capture is not
                // byte-exact and does not pretend to be.
                // `to_bytes` re-serialises and allocates, so it is inside the closure: with no
                // capture configured it never runs.
                self.observe(source, transport, Direction::In, || message.to_bytes());
                self.on_message(
                    *message,
                    source,
                    transport,
                    Some(id),
                    #[cfg(feature = "quic")]
                    quic_reply,
                )
                .await;
            }
            tcp::Event::FramingFailed { key } => {
                if let Some((id, admission_generation)) = self.pool.observation_generation(&key) {
                    self.observations.emit(connection_event(
                        key.clone(),
                        id,
                        admission_generation,
                        ConnectionState::Failed,
                    ));
                }
                // The stream half of a parse failure, counted against the transport that carried it
                // (§12). `Closed` follows and fails the transactions bound to the connection; this
                // is the *loss* — everything in flight on a stream whose framing is gone — which
                // until now was a `tracing::debug!` and nothing else.
                self.meters.parse_failure(key.transport);
            }
            tcp::Event::Pong { key, id } => {
                // First waiter for this connection, or nobody — a peer is entitled to send a
                // CRLF we did not ask for, and RFC 3261 §7.5 says to ignore it.
                let generation = ConnectionGeneration { key, id };
                if let Some(queue) = self.pong_waiters.get_mut(&generation)
                    && let Some(waiter) = queue.pop_front()
                {
                    // discard: the caller stopped waiting. A dropped receiver means nobody is listening
                    // for this answer, so nothing is lost and there is nothing worth counting.
                    let _ = waiter.send(Ok(None));
                }
            }
            #[cfg(feature = "tls")]
            tcp::Event::HandshakeFailed { key, id, detail } => {
                let admission_generation = self
                    .pool
                    .observation_generation(&key)
                    .filter(|(current, _)| *current == id)
                    .and_then(|(_, admission_generation)| admission_generation);
                self.observations.emit(connection_event(
                    key.clone(),
                    id,
                    admission_generation,
                    ConnectionState::Failed,
                ));
                // Authentication failure is terminal for this generation. Remove it now and fail
                // its transactions with the typed cause; the `Closed` emitted by the task wrapper
                // then becomes a stale close and has no second effect.
                if !self.pool.remove(&key, id) {
                    return;
                }
                let generation = ConnectionGeneration {
                    key: key.clone(),
                    id,
                };
                self.fail_transactions_on(&generation, Some(detail)).await;
            }
            #[cfg(feature = "quic")]
            tcp::Event::QuicClosed { key, id, detail } => {
                if !self.pool.remove(&key, id) {
                    return;
                }
                let generation = ConnectionGeneration {
                    key: key.clone(),
                    id,
                };
                for (transaction, bound) in &self.transaction_generations {
                    if bound == &generation
                        && let Some(client) = self.clients.get(transaction)
                    {
                        let failure = Error::Quic(crate::quic::QuicError::ConnectionClosed {
                            peer: key.peer.to_string(),
                            detail: detail.clone(),
                        });
                        let _ = client.failures.try_send(failure);
                    }
                }
                self.fail_transactions_on(&generation, None).await;
            }
            tcp::Event::Closed { key, id } => {
                // A retiring generation can report after a replacement with the same key has
                // already joined. Every side effect below belongs to the generation that closed,
                // so a stale report must not fail the replacement's transactions or keep-alives.
                if !self.pool.remove(&key, id) {
                    return;
                }
                let generation = ConnectionGeneration {
                    key: key.clone(),
                    id,
                };
                // A flow whose connection has gone is a failed flow, and saying so now beats
                // making the caller wait out its own timeout for something already known.
                if let Some(queue) = self.pong_waiters.remove(&generation) {
                    for waiter in queue {
                        // discard: the caller stopped waiting. A dropped receiver means nobody is listening
                        // for this answer, so nothing is lost and there is nothing worth counting.
                        let _ = waiter.send(Err(Error::ConnectionClosed));
                    }
                }
                self.fail_transactions_on(&generation, None).await;
            }
        }
    }

    /// Fail every transaction bound to a connection that has gone.
    ///
    /// The alternative is letting them time out, which means waiting up to 32 seconds to
    /// discover something already known — a bad experience and a resource leak.
    async fn fail_transactions_on(
        &mut self,
        closed: &ConnectionGeneration,
        tls_detail: Option<String>,
    ) {
        let affected: Vec<TransactionKey> = self
            .transaction_generations
            .iter()
            .filter(|(_, generation)| *generation == closed)
            // A server transaction that knows where the peer listens is not failed by the loss
            // of the connection its request arrived on: RFC 3261 §18.2.2 has it open a new one
            // to the advertised port, and the response is still deliverable.
            .filter(|(key, _)| !self.reconnect.contains_key(*key))
            .map(|(key, _)| key.clone())
            .collect();
        for key in affected {
            #[cfg(feature = "tls")]
            if let Some(detail) = &tls_detail
                && let Some(client) = self.clients.get(&key)
            {
                #[cfg(feature = "quic")]
                let failure = if closed.key.transport == TransportKind::Quic {
                    Error::Quic(crate::quic::QuicError::handshake(
                        closed.key.peer.to_string(),
                        detail.clone(),
                    ))
                } else {
                    Error::Tls(crate::tls::TlsError::Handshake {
                        peer: closed.key.peer.to_string(),
                        detail: detail.clone(),
                    })
                };
                #[cfg(not(feature = "quic"))]
                let failure = Error::Tls(crate::tls::TlsError::Handshake {
                    peer: closed.key.peer.to_string(),
                    detail: detail.clone(),
                });
                let _ = client.failures.try_send(failure);
            }
            #[cfg(not(feature = "tls"))]
            let _ = &tls_detail;
            let outputs = self.layer.on_transport_error(&key);
            self.perform(&key, outputs, None).await;
        }
    }

    async fn on_message(
        &mut self,
        message: Message,
        source: SocketAddr,
        transport: TransportKind,
        generation: Option<u64>,
        #[cfg(feature = "quic")] quic_reply: Option<crate::quic::Reply>,
    ) {
        let overload_response = match &message {
            Message::Response(response) => Some(response.clone()),
            Message::Request(_) => None,
        };
        // The one site inbound messages are counted, whichever transport carried them here: a
        // datagram arrives through `on_datagram` and a stream message through `on_net_event`, and
        // both funnel into this method (§12).
        self.meters
            .message_in(transport, matches!(message, Message::Response(_)));
        let message = apply_network_source(message, source);
        let observed_message = message.clone();

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
                self.observe_inbound(
                    observed_message,
                    source,
                    transport,
                    TransactionClass::ServerCreated,
                );
                self.destinations.insert(key.clone(), reply_to);
                if let Some(id) = generation {
                    self.transaction_generations.insert(
                        key.clone(),
                        ConnectionGeneration {
                            key: ConnectionKey::new(source, transport),
                            id,
                        },
                    );
                }
                #[cfg(feature = "quic")]
                self.remember_quic_reply(&key, quic_reply);
                self.handed_over
                    .insert(key.clone(), tokio::time::Instant::now());
                // §18.2.2's fallback only arises on a transport that has a connection to lose.
                if transport.reliability().is_reliable()
                    && transport != TransportKind::Quic
                    && let Some(advertised) = advertised
                {
                    self.reconnect.insert(key.clone(), advertised);
                }
                self.perform(&key, outputs, Some((source, transport))).await;
            }
            Dispatch::Matched { key, outputs } => {
                self.observe_inbound(
                    observed_message,
                    source,
                    transport,
                    TransactionClass::Matched,
                );
                self.observe_overload_response(source, overload_response.as_ref());
                self.perform(&key, outputs, Some((source, transport))).await;
            }
            Dispatch::Unmatched(message) => {
                self.observe_inbound(
                    observed_message,
                    source,
                    transport,
                    TransactionClass::Unmatched,
                );
                self.on_unmatched(message, source, transport);
            }
        }
    }

    fn on_unmatched(
        &mut self,
        message: Box<Message>,
        source: SocketAddr,
        transport: TransportKind,
    ) {
        tracing::debug!(%source, "message matched no transaction");
        // Counted before the question of whether anyone is watching: §16.7 makes an unmatched
        // response a forwarding element's business and a user agent's non-problem, and the rate
        // is worth knowing to either of them.
        if let Message::Response(response) = &*message {
            self.meters.unmatched_response();
            if let Some(sink) = &self.unmatched
                && sink
                    .try_send(Unmatched {
                        response: response.clone(),
                        source,
                        transport,
                    })
                    .is_err()
            {
                // A full watcher must not stop every endpoint timer while it catches up.
                self.meters.shed.unmatched.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    %source,
                    "unmatched-response watcher is not keeping up; dropped one"
                );
            }
            return;
        }

        // An unmatched ACK belongs to the application; anything else is noise it can still
        // choose to look at.
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
                // There is no transaction here to answer with a 503, so count the loss and name it.
                self.meters.shed.unmatched.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    %source,
                    method = %method,
                    "application queue full; an unmatched request was dropped"
                );
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
                self.meters.discard_unanswered();
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
            self.transaction_generations.remove(&key);
            #[cfg(feature = "quic")]
            self.quic_replies.remove(&key);
            // `reconnect` too. It is removed nowhere else but `Output::Terminated`, which an
            // abandoned transaction never reaches — so leaving it here would trade one
            // unbounded map for another.
            self.reconnect.remove(&key);
        }
    }

    async fn on_timers(&mut self) {
        let due = self.timers.take_due(tokio::time::Instant::now());
        for (key, timer) in due {
            // Counted here, where the timer fires, rather than after the socket call. A
            // retransmission the socket then refuses is still a retransmission this endpoint
            // decided to send; counting it later would mean a peer that stopped hearing us
            // produced a *falling* count (§12.2).
            self.meters.on_timer(timer);
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
                failures,
                reply,
            } => {
                let now =
                    tokio::time::Instant::now().saturating_duration_since(self.overload_epoch);
                let category = (self.overload_config.categorize)(&request);
                if !self.overload.admit(target.addr, category, now) {
                    self.meters.overload_rejection();
                    // discard: the caller dropped its wait; the rejection is already counted and
                    // no network request was lost.
                    let _ = reply.send(Err(Error::Overloaded { peer: target.addr }));
                    return;
                }
                let Some((key, outputs)) = self
                    .layer
                    .send_request(*request, target.transport.reliability())
                else {
                    // discard: the caller stopped waiting. A dropped receiver means nobody is listening
                    // for this answer, so nothing is lost and there is nothing worth counting.
                    let _ = reply.send(Err(Error::NoVia));
                    return;
                };
                self.destinations.insert(key.clone(), target);
                self.clients
                    .insert(key.clone(), ClientSink { events, failures });
                // discard: the caller stopped waiting. A dropped receiver means nobody is listening
                // for this answer, so nothing is lost and there is nothing worth counting.
                let _ = reply.send(Ok(key.clone()));
                self.perform(&key, outputs, None).await;
            }
            Command::Respond {
                key,
                response,
                sent,
            } => self.on_respond_command(key, response, sent).await,
            Command::Direct {
                request,
                target,
                sent,
            } => {
                let now =
                    tokio::time::Instant::now().saturating_duration_since(self.overload_epoch);
                let category = (self.overload_config.categorize)(&request);
                if !self.overload.admit(target.addr, category, now) {
                    self.meters.overload_rejection();
                    // discard: the caller dropped its wait; the rejection is already counted and
                    // no network request was lost.
                    let _ = sent.send(Err(Error::Overloaded { peer: target.addr }));
                    return;
                }
                let method = request.method.clone();
                let message = Message::Request(*request);
                self.observe_message(
                    message.clone(),
                    target.addr,
                    target.transport,
                    MessageDirection::Outbound,
                    TransactionClass::Direct,
                );
                let bytes = message.to_bytes();
                self.observe_out(&bytes, &target, false);
                let result = self.transmit(bytes, target, false, None).await.map(|_| ());
                if result.is_err() {
                    // The same fact as the transaction path's site above, on the one request that
                    // has no transaction (§12.3). Deliberately *not* also
                    // `discard_send_failure`: that field is the transaction path's aggregate, and
                    // an ACK for a 2xx never had a transaction to fail.
                    self.meters.unsent(&method);
                }
                // discard: the caller stopped waiting. A dropped receiver means nobody is listening
                // for this answer, so nothing is lost and there is nothing worth counting.
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
                // discard: the caller stopped waiting. A dropped receiver means nobody is listening
                // for this answer, so nothing is lost and there is nothing worth counting.
                let _ = reply.send(
                    clients
                        + servers
                        + self.destinations.len()
                        + self.transaction_generations.len()
                        + {
                            #[cfg(feature = "quic")]
                            {
                                self.quic_replies.len()
                            }
                            #[cfg(not(feature = "quic"))]
                            {
                                0
                            }
                        }
                        + self.reconnect.len()
                        + self.handed_over.len(),
                );
            }
            Command::Shutdown => {}
        }
    }

    async fn on_respond_command(
        &mut self,
        key: TransactionKey,
        response: Box<Response>,
        sent: oneshot::Sender<Result<()>>,
    ) {
        // Nothing is removed from `handed_over` here, and that is the point. A provisional
        // response is not an answer: an application that sends 180 Ringing and then wedges has a
        // transaction sitting in `Proceeding`, which RFC 3261 §17.2.1 gives no timer either.
        if self.layer.server_request(&key).is_none() {
            // No transaction to answer on. Reporting success here would tell an application its
            // 200 OK went out while the caller heard nothing.
            // discard: the caller stopped waiting, so nothing is lost or worth counting.
            let _ = sent.send(Err(Error::NoTransaction));
            return;
        }
        let outputs = self.layer.send_response(&key, *response);
        // The success reported here is produced by the send: consuming `Performed` is the only
        // way to obtain the `Ok`, so reversing these statements does not compile (`X-36`).
        let performed = self.perform(&key, outputs, None).await;
        // discard: the caller stopped waiting, so nothing is lost or worth counting.
        let _ = sent.send(performed.into_result());
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
                    let mut message = *message;
                    if let Message::Response(response) = &mut message
                        && let Some(request) = self.layer.server_request(key).cloned()
                    {
                        self.decorate_overload_response(response, &request);
                    }
                    let target =
                        self.destinations.get(key).cloned().or_else(|| {
                            origin.map(|(addr, transport)| Target::new(addr, transport))
                        });
                    let Some(target) = target else {
                        self.meters.discard_no_destination();
                        tracing::warn!("no destination for a message the transaction wants sent");
                        continue;
                    };
                    // Kept before `to_bytes` consumes the message: a failed transmit is counted by
                    // method (§12.3), and after this line the method is no longer reachable.
                    let method = match &message {
                        Message::Request(request) => Some(request.method.clone()),
                        Message::Response(_) => None,
                    };
                    let is_response = method.is_none();
                    self.observe_message(
                        message.clone(),
                        target.addr,
                        target.transport,
                        MessageDirection::Outbound,
                        if is_response {
                            TransactionClass::Matched
                        } else {
                            TransactionClass::ClientCreated
                        },
                    );
                    let bytes = message.to_bytes();
                    let addr = target.addr;
                    self.observe_out(&bytes, &target, is_response);
                    let fallback = self.reconnect.get(key).cloned();
                    #[cfg(feature = "quic")]
                    let transmitted = if is_response && target.transport == TransportKind::Quic {
                        match self.quic_replies.get(key).cloned() {
                            Some(reply) => reply
                                .send(bytes)
                                .await
                                .map(|()| self.transaction_generations.get(key).cloned())
                                .map_err(|_| Error::ConnectionClosed),
                            None => Err(Error::ConnectionClosed),
                        }
                    } else {
                        self.transmit(bytes, target, is_response, fallback).await
                    };
                    #[cfg(not(feature = "quic"))]
                    let transmitted = self.transmit(bytes, target, is_response, fallback).await;
                    match transmitted {
                        Ok(Some(generation)) => {
                            self.transaction_generations.insert(key.clone(), generation);
                        }
                        Ok(None) => {
                            self.transaction_generations.remove(key);
                        }
                        Err(error) => {
                            self.meters.discard_send_failure();
                            // And by method, when it was a request (§12.3). This is where the
                            // wire is actually missed: `Handle::send` has already returned `Ok`
                            // with the transaction key by now, so counting at that hand-off would
                            // miss every refused connection, unreachable peer and over-MTU
                            // datagram — which is the whole of "why did that call linger".
                            if let Some(method) = &method {
                                self.meters.unsent(method);
                            }
                            tracing::warn!(%error, %addr, "send failed");
                            if let Some(client) = self.clients.get(key) {
                                // One transport failure terminates this transaction, so one
                                // bounded slot is sufficient. A full/closed slot means the caller
                                // has already stopped listening.
                                let _ = client.failures.try_send(error);
                            }
                            let outputs = self.layer.on_transport_error(key);
                            return Box::pin(self.perform(key, outputs, origin)).await;
                        }
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
                    self.transaction_generations.remove(key);
                    #[cfg(feature = "quic")]
                    self.quic_replies.remove(key);
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

    /// Hand one observed message to the capture, if one is running (§13).
    ///
    /// Called from the driver loop, which is what makes the sequence number the capture stamps
    /// meaningful: the *order* is decided here, at the point the bytes crossed the boundary, and the
    /// write happens elsewhere. Costs one `Option` check when no capture is configured.
    fn observe(
        &mut self,
        peer: SocketAddr,
        transport: TransportKind,
        direction: Direction,
        bytes: impl FnOnce() -> Bytes,
    ) {
        // `bytes` is a closure so that an endpoint with no capture pays nothing: see
        // `Capture::observe_if_capturing`, which is where the guard and its test live.
        Capture::observe_if_capturing(
            self.capture.as_mut(),
            &self.meters,
            self.local_addr,
            peer,
            transport,
            direction,
            bytes,
        );
    }

    fn observe_message(
        &self,
        message: Message,
        peer: SocketAddr,
        transport: TransportKind,
        direction: MessageDirection,
        transaction: TransactionClass,
    ) {
        self.observations
            .emit(EndpointObservation::Message(Box::new(MessageObservation {
                message,
                local: self.local_addr,
                peer,
                transport,
                direction,
                transaction,
            })));
    }

    fn observe_inbound(
        &self,
        message: Message,
        peer: SocketAddr,
        transport: TransportKind,
        transaction: TransactionClass,
    ) {
        self.observe_message(
            message,
            peer,
            transport,
            MessageDirection::Inbound,
            transaction,
        );
    }

    /// Count and capture a SIP message on its way out.
    ///
    /// The one site outbound messages are counted, so §12.2's "exactly one increment site per
    /// counter" holds. Deliberately *not* inside [`Driver::transmit`]: that also carries keep-alives,
    /// which are not SIP messages and must not be counted as requests.
    fn observe_out(&mut self, bytes: &Bytes, target: &Target, is_response: bool) {
        self.meters.message_out(target.transport, is_response);
        // Already serialised — the send needs these bytes either way — so the clone is a refcount.
        self.observe(target.addr, target.transport, Direction::Out, || {
            bytes.clone()
        });
    }

    /// Put bytes on the wire that are not a SIP message.
    ///
    /// A keep-alive is not a request and must not be treated as one: no MTU refusal (a STUN
    /// header is 20 bytes), no transaction, no `Via`. It reuses [`Driver::transmit`] so a flow's
    /// ping travels over the *same connection* its requests do — which is the whole of RFC 5626
    /// §4.4, since a ping on a second connection tests a flow nobody is using.
    async fn transmit_raw(
        &mut self,
        bytes: Bytes,
        target: &Target,
    ) -> Result<Option<ConnectionGeneration>> {
        self.transmit(bytes, target.clone(), true, None).await
    }

    #[cfg(feature = "quic")]
    fn remember_quic_reply(&mut self, key: &TransactionKey, reply: Option<crate::quic::Reply>) {
        if let Some(reply) = reply {
            self.quic_replies.insert(key.clone(), reply);
        }
    }

    #[cfg(feature = "quic")]
    async fn transmit_quic(
        &mut self,
        bytes: Bytes,
        target: &Target,
        is_response: bool,
    ) -> Result<Option<ConnectionGeneration>> {
        if is_response {
            return Err(Error::ConnectionClosed);
        }
        let Some(client) = self.quic_client.clone() else {
            return Err(Error::UnsupportedTransport(
                "QUIC (no client configuration, so no outbound connection can be verified)",
            ));
        };
        let Some(endpoint) = self.quic_endpoint.clone() else {
            return Err(Error::UnsupportedTransport("QUIC (no local endpoint)"));
        };
        let key = target.connection();
        let name = target
            .verify_as
            .as_deref()
            .map_or_else(|| target.addr.ip().to_string(), str::to_owned);
        let id = self
            .pool
            .send_quic_generation(&key, &name, &client, &endpoint, bytes)
            .await?;
        Ok(Some(ConnectionGeneration { key, id }))
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
    ) -> Result<Option<ConnectionGeneration>> {
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
                Ok(None)
            }
            TransportKind::Tcp => {
                let key = target.connection();
                if is_response
                    && let Some(id) = self
                        .pool
                        .send_on_existing_generation(&key, bytes.clone())
                        .await
                {
                    return Ok(Some(ConnectionGeneration { key, id }));
                }
                // The connection is gone. RFC 3261 §18.2.2 sends the response to the address
                // the request came from at the port the sender said it listens on — not back
                // at the ephemeral port it dialled out from, where nothing is accepting.
                let key = match (is_response, &fallback) {
                    (true, Some(advertised)) => advertised.connection(),
                    _ => key,
                };
                let id = self.pool.send_generation(&key, bytes).await?;
                Ok(Some(ConnectionGeneration { key, id }))
            }
            #[cfg(feature = "tls")]
            TransportKind::Tls => {
                // Answering on the connection the request arrived over comes first, and needs
                // no client configuration at all — a pure TLS server has no reason to hold
                // one, and requiring it would leave such a server unable to reply.
                let key = target.connection();
                if is_response
                    && let Some(id) = self
                        .pool
                        .send_on_existing_generation(&key, bytes.clone())
                        .await
                {
                    return Ok(Some(ConnectionGeneration { key, id }));
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
                let id = self
                    .pool
                    .send_tls_generation(&key, &name, &client, bytes)
                    .await?;
                Ok(Some(ConnectionGeneration { key, id }))
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
                if let Some(id) = self
                    .pool
                    .send_on_existing_generation(&key, bytes.clone())
                    .await
                {
                    return Ok(Some(ConnectionGeneration { key, id }));
                }
                let authority = target.verify_as.as_deref().map_or_else(
                    || target.addr.to_string(),
                    |name| format!("{name}:{}", target.addr.port()),
                );
                let id = self
                    .pool
                    .send_ws_generation(
                        &key,
                        &authority,
                        self.ws_keepalive,
                        #[cfg(feature = "wss")]
                        self.tls_client.as_ref(),
                        bytes,
                    )
                    .await?;
                Ok(Some(ConnectionGeneration { key, id }))
            }
            #[cfg(feature = "quic")]
            TransportKind::Quic => self.transmit_quic(bytes, &target, is_response).await,
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
        if let Some(client) = self.clients.get(key) {
            // The receiver is gone: the application dropped its `Responses` before the transaction
            // finished. Legitimate — a caller that stopped caring is allowed to — but it means an
            // outcome went nowhere, and nothing retransmits an event, so it is counted rather than
            // discarded in silence (§12.1).
            if client.events.send(event).await.is_err() {
                self.meters.discard_transaction_event();
                tracing::debug!(
                    "a transaction event had no receiver; the caller stopped listening"
                );
            }
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
                        self.meters.shed.acks.fetch_add(1, Ordering::Relaxed);
                        tracing::error!(
                            %source,
                            "application queue full; an ACK was dropped and cannot be refused — \
                             the dialog it would have completed will not be reaped"
                        );
                    } else {
                        self.meters.shed.requests.fetch_add(1, Ordering::Relaxed);
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
        self.server_overloaded_until =
            Some(tokio::time::Instant::now() + self.overload_config.validity);
        let outputs = self.layer.send_response(key, builder.build());
        Box::pin(self.perform(key, outputs, None)).await;
    }

    /// Accept feedback only after the transaction layer has authenticated it by matching a live
    /// client transaction. An unmatched response is application data, not controller input.
    fn observe_overload_response(&mut self, source: SocketAddr, response: Option<&Response>) {
        if !self.overload_config.advertise {
            return;
        }
        if let Some(response) = response {
            let now = tokio::time::Instant::now().saturating_duration_since(self.overload_epoch);
            self.overload.observe(source, response, now);
        }
    }

    /// Decorate every server response with the queue detector's current state.
    fn decorate_overload_response(&mut self, response: &mut Response, request: &Request) {
        let now = tokio::time::Instant::now();
        let active_for = self
            .server_overloaded_until
            .and_then(|until| until.checked_duration_since(now));
        let (feedback, validity) = match active_for {
            Some(remaining) if !remaining.is_zero() => {
                let millis = u64::try_from(remaining.as_millis().max(1)).unwrap_or(u64::MAX);
                (
                    self.overload_config.feedback,
                    std::time::Duration::from_millis(millis),
                )
            }
            _ => {
                self.server_overloaded_until = None;
                let stopped = match self.overload_config.feedback {
                    crate::OverloadFeedback::Loss(_) => crate::OverloadFeedback::Loss(0),
                    crate::OverloadFeedback::Rate(_) => crate::OverloadFeedback::Rate(0),
                };
                (stopped, std::time::Duration::ZERO)
            }
        };
        self.overload_sequence = if self.overload_sequence >= 999_999_999_999 {
            1
        } else {
            self.overload_sequence.saturating_add(1)
        };
        if let Some(sequence) =
            sipx_sip::headers::OverloadSequence::from_integer(self.overload_sequence)
        {
            crate::overload::add_feedback(response, request, feedback, validity, sequence);
        }
    }

    fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        // Never resolves; the `if` guard in `select!` keeps this branch disabled anyway.
        None => std::future::pending().await,
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
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::net::TcpStream;
    #[cfg(any(feature = "tls", feature = "ws"))]
    use tokio::sync::Semaphore;
    use tokio::sync::mpsc;

    #[cfg(any(feature = "tls", feature = "ws"))]
    use super::{Adopt, HandshakeObservation, HandshakeRuntime};
    use super::{Background, Driver, Message, ShutdownState, Target, TransportKind};

    const IDENTIFIER_SAMPLE_SIZE: u64 = 4096;

    fn bit_counts(values: impl IntoIterator<Item = u64>) -> [usize; 64] {
        let mut counts = [0; 64];
        for value in values {
            for (bit, count) in counts.iter_mut().enumerate() {
                *count += usize::from(value & (1_u64 << bit) != 0);
            }
        }
        counts
    }

    fn assert_full_width(counts: &[usize; 64], subject: &str) {
        for (bit, ones) in counts.iter().copied().enumerate() {
            assert!(
                (1664..=2432).contains(&ones), // 128 positions * 2 * exp(-2 * 384^2 / 4096) < 1.4e-29.
                "{subject} bit {bit} had {ones} ones in {IDENTIFIER_SAMPLE_SIZE} samples"
            );
        }
    }

    /// RFC 3261 §8.1.1.7 requires the magic cookie. The remaining sixteen hexadecimal digits
    /// are the 64 random bits promised by `sip-transport.md` §7; checking every bit's balance
    /// catches a truncated value and a counter whose high bits never change.
    #[test]
    fn via_branch_keeps_the_cookie_and_all_sixty_four_random_bits() {
        let values = (0..IDENTIFIER_SAMPLE_SIZE).map(|_| {
            let branch = super::new_branch();
            let random = branch
                .strip_prefix("z9hG4bK")
                .expect("the RFC 3261 magic cookie");
            assert_eq!(random.len(), 16, "exactly 64 bits in hexadecimal");
            assert!(
                random
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
                "the random portion is canonical lowercase hexadecimal: {random}"
            );
            u64::from_str_radix(random, 16).expect("the generator wrote hexadecimal")
        });
        assert_full_width(&bit_counts(values), "Via branch");
    }

    /// The bound on `branch_with_rng` is the non-statistical assertion: a generator that only
    /// implements `RngCore` cannot be used here, however plausible its sample looks.
    #[test]
    fn via_branch_requires_a_cryptographic_rng_by_construction() {
        fn draw<R: rand::CryptoRng + ?Sized>(rng: &mut R) -> String {
            super::branch_with_rng(rng)
        }

        let branch = draw(&mut rand::rng());
        assert!(branch.starts_with("z9hG4bK"));
    }

    /// The statistical guard is not the cryptographic proof; this shows that it detects the
    /// cheaper counter substitution which the compiler's `CryptoRng` bound independently refuses.
    #[test]
    fn the_width_guard_rejects_a_counter() {
        let counts = bit_counts(0..IDENTIFIER_SAMPLE_SIZE);
        assert!(
            counts.iter().any(|ones| !(1664..=2432).contains(ones)),
            "a 12-bit counter must not look like a 64-bit generator"
        );
    }

    async fn driver_with_pool(
        pool: crate::tcp::Pool,
        net: mpsc::Receiver<crate::tcp::Event>,
    ) -> Driver {
        let socket = Arc::new(
            tokio::net::UdpSocket::bind("127.0.0.1:0")
                .await
                .expect("UDP binds"),
        );
        let local_addr = socket.local_addr().expect("local address");
        let (_commands_tx, commands) = mpsc::channel(8);
        let (incoming, _incoming_rx) = mpsc::channel(8);
        let (_accepts_tx, accepts) = mpsc::channel(8);
        let (adopt, adopts) = mpsc::channel(8);
        let meters = Arc::new(crate::counters::Meters::default());
        let admission = Arc::new(crate::policy::SourceAdmission::default());
        let observations = Arc::new(crate::policy::ObservationHub::new(Arc::clone(&meters)));
        Driver {
            socket,
            layer: sipx_sip::transaction::TransactionLayer::new(sipx_sip::Timers::default()),
            timers: crate::timers::TimerQueue::new(),
            destinations: std::collections::HashMap::new(),
            transaction_generations: std::collections::HashMap::new(),
            handed_over: std::collections::HashMap::new(),
            reconnect: std::collections::HashMap::new(),
            unanswered_limit: Duration::from_secs(60),
            overload: crate::overload::Controller::new(5, 10, 1024),
            overload_config: crate::OverloadConfig::default(),
            overload_epoch: tokio::time::Instant::now(),
            overload_sequence: 0,
            server_overloaded_until: None,
            clients: std::collections::HashMap::new(),
            incoming,
            commands,
            net,
            accepts,
            adopts,
            _adopt: adopt,
            #[cfg(feature = "tls")]
            tls_client: None,
            #[cfg(feature = "ws")]
            ws_keepalive: Duration::from_secs(60),
            #[cfg(feature = "quic")]
            quic_client: None,
            #[cfg(feature = "quic")]
            quic_endpoint: None,
            pool,
            limits: sipx_sip::Limits::stream(),
            mtu: 1300,
            meters,
            admission,
            observations,
            capture: None,
            local_addr,
            unmatched: None,
            stun_waiters: std::collections::HashMap::new(),
            pong_waiters: std::collections::HashMap::new(),
            #[cfg(feature = "quic")]
            quic_replies: std::collections::HashMap::new(),
            background: Background::new(),
            shutdown: Arc::new(ShutdownState::default()),
        }
    }

    #[tokio::test]
    async fn stale_close_does_not_fail_a_transaction_on_the_live_generation() {
        use sipx_sip::transaction::Reliability;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("TCP binds");
        let address = listener.local_addr().expect("listener address");
        let peer_socket = TcpStream::connect(address).await.expect("peer connects");
        let (server_socket, peer) = listener.accept().await.expect("connection accepts");
        let key = crate::ConnectionKey::new(peer, TransportKind::Tcp);
        let (net_tx, net_rx) = mpsc::channel(8);
        let mut pool = crate::tcp::Pool::new(
            crate::tcp::PoolConfig::default(),
            sipx_sip::Limits::stream(),
            net_tx,
        );
        pool.accept(server_socket, peer);
        assert!(pool.holds(&key), "the live generation is installed");

        let mut driver = driver_with_pool(pool, net_rx).await;
        let parsed = sipx_sip::parse_datagram(
            bytes::Bytes::from_static(
                b"OPTIONS sip:a@example.com SIP/2.0\r\n\
                  Via: SIP/2.0/TCP 127.0.0.1:5555;branch=z9hG4bKstale\r\n\
                  To: <sip:a@example.com>\r\n\
                  From: <sip:b@example.net>;tag=1\r\n\
                  Call-ID: stale-close@example.net\r\n\
                  CSeq: 1 OPTIONS\r\n\
                  Max-Forwards: 70\r\n\
                  Content-Length: 0\r\n\r\n",
            ),
            &sipx_sip::Limits::datagram(),
        )
        .expect("request parses");
        let Message::Request(request) = parsed else {
            panic!("expected a request");
        };
        let (transaction, _outputs) = driver
            .layer
            .send_request(request, Reliability::Reliable)
            .expect("transaction starts");
        driver
            .destinations
            .insert(transaction.clone(), Target::new(peer, TransportKind::Tcp));
        let live_id = driver.pool.generation(&key).expect("live generation");
        driver.transaction_generations.insert(
            transaction.clone(),
            super::ConnectionGeneration {
                key: key.clone(),
                id: live_id,
            },
        );
        let (client_events, mut received) = mpsc::channel(8);
        let (failures, _failure_rx) = mpsc::channel(1);
        driver.clients.insert(
            transaction.clone(),
            super::ClientSink {
                events: client_events,
                failures,
            },
        );

        // Generation zero predates every real pool entry (IDs begin at one), modelling an old
        // task's delayed close after the current generation and transaction were installed.
        driver
            .on_net_event(crate::tcp::Event::Closed {
                key: key.clone(),
                id: 0,
            })
            .await;

        assert!(driver.pool.holds(&key), "the live connection survives");
        assert_eq!(driver.layer.len(), (1, 0), "the transaction stays live");
        assert!(driver.destinations.contains_key(&transaction));
        assert!(driver.clients.contains_key(&transaction));
        assert!(
            matches!(received.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
            "the client receives no stale transport error"
        );
        driver.pool.shutdown().await;
        drop(peer_socket);
    }

    #[tokio::test]
    async fn retiring_current_generation_fails_its_transaction_and_pong_waiter_once() {
        use sipx_sip::transaction::Reliability;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("TCP binds");
        let address = listener.local_addr().expect("listener address");
        let peer_socket = TcpStream::connect(address).await.expect("peer connects");
        let (server_socket, peer) = listener.accept().await.expect("connection accepts");
        let key = crate::ConnectionKey::new(peer, TransportKind::Tcp);
        let (net_tx, net_rx) = mpsc::channel(8);
        let mut pool = crate::tcp::Pool::new(
            crate::tcp::PoolConfig {
                idle_timeout: Duration::ZERO,
                ..crate::tcp::PoolConfig::default()
            },
            sipx_sip::Limits::stream(),
            net_tx,
        );
        pool.accept(server_socket, peer);
        let id = pool.generation(&key).expect("generation");
        let mut driver = driver_with_pool(pool, net_rx).await;
        let parsed = sipx_sip::parse_datagram(
            bytes::Bytes::from_static(
                b"OPTIONS sip:a@example.com SIP/2.0\r\n\
                  Via: SIP/2.0/TCP 127.0.0.1:5555;branch=z9hG4bKretire\r\n\
                  To: <sip:a@example.com>\r\n\
                  From: <sip:b@example.net>;tag=1\r\n\
                  Call-ID: retire@example.net\r\n\
                  CSeq: 1 OPTIONS\r\n\
                  Max-Forwards: 70\r\n\
                  Content-Length: 0\r\n\r\n",
            ),
            &sipx_sip::Limits::datagram(),
        )
        .expect("request parses");
        let Message::Request(request) = parsed else {
            panic!("expected request");
        };
        let (transaction, _outputs) = driver
            .layer
            .send_request(request, Reliability::Reliable)
            .expect("transaction starts");
        driver
            .destinations
            .insert(transaction.clone(), Target::new(peer, TransportKind::Tcp));
        let generation = super::ConnectionGeneration {
            key: key.clone(),
            id,
        };
        driver
            .transaction_generations
            .insert(transaction.clone(), generation.clone());
        let (client_events, mut received) = mpsc::channel(8);
        let (failures, _failure_rx) = mpsc::channel(1);
        driver.clients.insert(
            transaction,
            super::ClientSink {
                events: client_events,
                failures,
            },
        );
        let (pong, pong_result) = tokio::sync::oneshot::channel();
        driver
            .pong_waiters
            .entry(generation)
            .or_default()
            .push_back(pong);

        assert_eq!(driver.pool.evict_idle(), vec![key.clone()]);
        driver
            .on_net_event(crate::tcp::Event::Closed {
                key: key.clone(),
                id,
            })
            .await;
        assert!(matches!(
            received.recv().await,
            Some(sipx_sip::transaction::TuEvent::TransportError)
        ));
        assert!(matches!(
            pong_result.await,
            Ok(Err(crate::Error::ConnectionClosed))
        ));
        assert!(!driver.pool.holds(&key));

        // A duplicated close is stale after the first acknowledgement and has no second effect.
        driver
            .on_net_event(crate::tcp::Event::Closed { key, id })
            .await;
        driver.pool.shutdown().await;
        drop(peer_socket);
    }

    #[tokio::test]
    async fn queued_old_pong_cannot_answer_the_replacement_generation_waiter() {
        let (events, net_rx) = mpsc::channel(8);
        let pool = crate::tcp::Pool::new(
            crate::tcp::PoolConfig::default(),
            sipx_sip::Limits::stream(),
            events,
        );
        let mut driver = driver_with_pool(pool, net_rx).await;
        let key = crate::ConnectionKey::new(
            "127.0.0.1:59999".parse().expect("address"),
            TransportKind::Tcp,
        );
        let replacement_id = 2;
        let (answered, mut answer) = tokio::sync::oneshot::channel();
        driver
            .pong_waiters
            .entry(super::ConnectionGeneration {
                key: key.clone(),
                id: replacement_id,
            })
            .or_default()
            .push_back(answered);

        driver
            .on_net_event(crate::tcp::Event::Pong {
                key: key.clone(),
                id: 1,
            })
            .await;
        assert!(matches!(
            answer.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        driver
            .on_net_event(crate::tcp::Event::Pong {
                key,
                id: replacement_id,
            })
            .await;
        assert!(matches!(answer.await, Ok(Ok(None))));
        driver.pool.shutdown().await;
    }

    fn handle_with_shutdown_barrier(
        commands: mpsc::Sender<super::Command>,
        shutdown: Arc<ShutdownState>,
    ) -> super::Handle {
        let meters = Arc::new(crate::counters::Meters::default());
        super::Handle {
            commands,
            shutdown,
            local_addr: "127.0.0.1:5060".parse().expect("address"),
            meters: Arc::clone(&meters),
            admission: Arc::new(crate::policy::SourceAdmission::default()),
            observations: Arc::new(crate::policy::ObservationHub::new(meters)),
            request_policy: None,
            #[cfg(feature = "tls")]
            tls_addr: None,
            #[cfg(feature = "tls")]
            server_identity: None,
            #[cfg(feature = "ws")]
            ws_addr: None,
            #[cfg(feature = "wss")]
            wss_addr: None,
            #[cfg(feature = "quic")]
            quic_addr: None,
            #[cfg(feature = "ws")]
            ws_sent_by: Arc::from("shutdown.invalid"),
            advertise_overload: false,
            sent_by: Arc::new("127.0.0.1".to_owned()),
            sent_by_port: 5060,
        }
    }

    #[tokio::test]
    async fn caller_arriving_after_command_closure_still_waits_for_cleanup_completion() {
        let (commands, mut received) = mpsc::channel(8);
        let shutdown = Arc::new(ShutdownState::default());
        let handle = handle_with_shutdown_barrier(commands, Arc::clone(&shutdown));
        let (receiver_closed, closed) = tokio::sync::oneshot::channel();
        let (release_cleanup, cleanup_released) = tokio::sync::oneshot::channel();
        let driver = tokio::spawn(async move {
            assert!(matches!(
                received.recv().await,
                Some(super::Command::Shutdown)
            ));
            received.close();
            receiver_closed.send(()).expect("test remains present");
            cleanup_released.await.expect("cleanup is released");
            shutdown.complete();
        });

        let first = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.shutdown().await })
        };
        closed.await.expect("driver closed command receiver");
        let late = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.shutdown().await })
        };
        tokio::task::yield_now().await;
        assert!(
            !late.is_finished(),
            "late caller waits on the durable barrier after send fails"
        );

        release_cleanup.send(()).expect("driver remains present");
        first.await.expect("first shutdown returns after cleanup");
        late.await.expect("late shutdown returns after cleanup");
        driver.await.expect("driver completes");
    }

    #[cfg(any(feature = "tls", feature = "ws"))]
    const DEADLINE: Duration = Duration::from_millis(150);

    #[cfg(any(feature = "tls", feature = "ws"))]
    fn handshake_runtime(
        limit: usize,
    ) -> (
        Background,
        HandshakeRuntime,
        mpsc::UnboundedReceiver<HandshakeObservation>,
    ) {
        let owner = Background::new();
        let (observations, observed) = mpsc::unbounded_channel();
        let runtime = HandshakeRuntime {
            deadline: DEADLINE,
            permits: Arc::new(Semaphore::new(limit)),
            owner: owner.clone(),
            observations: Some(observations),
        };
        (owner, runtime, observed)
    }

    async fn wait_for_observation(
        observed: &mut mpsc::UnboundedReceiver<HandshakeObservation>,
        expected: HandshakeObservation,
    ) {
        assert_eq!(
            observed.recv().await.expect("listener remains alive"),
            expected
        );
    }

    #[cfg(any(feature = "tls", feature = "ws"))]
    async fn wait_for_available(runtime: &HandshakeRuntime, expected: usize) {
        for _ in 0..512 {
            if runtime.permits.available_permits() == expected {
                return;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(runtime.permits.available_permits(), expected);
    }

    #[cfg(any(feature = "tls", feature = "ws"))]
    async fn wait_for_eof(stream: &TcpStream) {
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut byte = [0u8; 1];
            loop {
                stream.readable().await.expect("socket remains readable");
                match stream.try_read(&mut byte) {
                    Ok(0) => return,
                    Ok(_) => panic!("a refused incomplete handshake produced bytes"),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::ConnectionReset
                                | std::io::ErrorKind::ConnectionAborted
                                | std::io::ErrorKind::BrokenPipe
                        ) =>
                    {
                        return;
                    }
                    Err(error) => panic!("unexpected read error: {error}"),
                }
            }
        })
        .await
        .expect("peer closes within the configured handshake deadline");
    }

    #[cfg(any(feature = "tls", feature = "ws"))]
    fn open_source_policy() -> (
        Arc<crate::policy::SourceAdmission>,
        Arc<crate::counters::Meters>,
    ) {
        (
            Arc::new(crate::policy::SourceAdmission::default()),
            Arc::new(crate::counters::Meters::default()),
        )
    }

    /// X18: incomplete upgrades have one endpoint-wide budget and an observed admission barrier.
    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn websocket_handshake_budget_has_deterministic_admission_and_reclamation() {
        let (owner, runtime, mut observed) = handshake_runtime(2);
        let (admission, meters) = open_source_policy();
        let (adopt, mut adopted) = mpsc::channel::<Adopt>(8);
        let address = super::listen_ws(
            "127.0.0.1".parse().expect("loopback"),
            0,
            Duration::from_secs(60),
            sipx_sip::Limits::stream(),
            &adopt,
            &runtime,
            admission,
            meters,
        )
        .await
        .expect("listener binds");

        let first = TcpStream::connect(address).await.expect("first connects");
        wait_for_observation(&mut observed, HandshakeObservation::Admitted).await;
        wait_for_available(&runtime, 1).await;
        let second = TcpStream::connect(address).await.expect("second connects");
        wait_for_observation(&mut observed, HandshakeObservation::Admitted).await;
        wait_for_available(&runtime, 0).await;

        for _ in 0..16 {
            let refused = TcpStream::connect(address).await.expect("excess connects");
            wait_for_observation(&mut observed, HandshakeObservation::Refused).await;
            wait_for_eof(&refused).await;
        }

        wait_for_eof(&first).await;
        wait_for_eof(&second).await;
        wait_for_available(&runtime, 2).await;

        let stream = TcpStream::connect(address)
            .await
            .expect("connects after deadline");
        let socket = crate::ws::connect(stream, &address.to_string(), "/", false)
            .await
            .expect("released permit admits an upgrade");
        let adoption = adopted.recv().await.expect("upgraded socket is adopted");
        drop(adoption);
        drop(socket);
        owner.shutdown().await;
    }

    /// X18: TLS and WebSocket listeners draw from the same directly observed permit.
    #[cfg(all(feature = "tls", feature = "ws"))]
    #[tokio::test]
    async fn tls_and_websocket_share_one_deterministic_handshake_budget() {
        use sipx_testkit::certs::Ca;

        use crate::tls::{Identity, ServerTls};

        let ca = Ca::new();
        let (certificate, key) = ca.issue_for("localhost");
        let identity =
            Identity::from_pem(certificate.as_bytes(), key.as_bytes()).expect("an identity");
        let (owner, runtime, mut observed) = handshake_runtime(1);
        let (admission, meters) = open_source_policy();
        let (adopt, mut adopted) = mpsc::channel::<Adopt>(8);
        let (_identity_tx, identity_rx) = tokio::sync::watch::channel(None);
        let tls_address = super::listen_tls(
            "127.0.0.1".parse().expect("loopback"),
            0,
            super::ServerHandshakePolicy::new(
                ServerTls::new(identity).expect("a server"),
                identity_rx,
            ),
            &adopt,
            &runtime,
            Arc::clone(&admission),
            Arc::clone(&meters),
        )
        .await
        .expect("TLS listener binds");
        let ws_address = super::listen_ws(
            "127.0.0.1".parse().expect("loopback"),
            0,
            Duration::from_secs(60),
            sipx_sip::Limits::stream(),
            &adopt,
            &runtime,
            admission,
            meters,
        )
        .await
        .expect("WebSocket listener binds");

        let partial_tls = TcpStream::connect(tls_address).await.expect("TLS connects");
        wait_for_observation(&mut observed, HandshakeObservation::Admitted).await;
        wait_for_available(&runtime, 0).await;
        let refused_ws = TcpStream::connect(ws_address)
            .await
            .expect("WebSocket TCP connects");
        wait_for_observation(&mut observed, HandshakeObservation::Refused).await;
        wait_for_eof(&refused_ws).await;

        wait_for_eof(&partial_tls).await;
        wait_for_available(&runtime, 1).await;

        let stream = TcpStream::connect(ws_address)
            .await
            .expect("connects after deadline");
        let socket = crate::ws::connect(stream, &ws_address.to_string(), "/", false)
            .await
            .expect("released shared permit admits WebSocket");
        let adoption = adopted.recv().await.expect("upgraded socket is adopted");
        drop(adoption);
        drop(socket);
        owner.shutdown().await;
    }

    /// X18: WSS keeps its single permit across both TLS and HTTP upgrade phases.
    #[cfg(feature = "wss")]
    #[tokio::test]
    async fn wss_handshake_budget_has_deterministic_admission_and_reclamation() {
        use sipx_testkit::certs::Ca;

        use crate::tls::{Identity, ServerTls};

        let ca = Ca::new();
        let (certificate, key) = ca.issue_for("localhost");
        let identity =
            Identity::from_pem(certificate.as_bytes(), key.as_bytes()).expect("an identity");
        let (owner, runtime, mut observed) = handshake_runtime(1);
        let (admission, meters) = open_source_policy();
        let (adopt, _adopted) = mpsc::channel::<Adopt>(8);
        let (_identity_tx, identity_rx) = tokio::sync::watch::channel(None);
        let address = super::listen_wss(
            "127.0.0.1".parse().expect("loopback"),
            0,
            super::ServerHandshakePolicy::new(
                ServerTls::new(identity).expect("a server"),
                identity_rx,
            ),
            Duration::from_secs(60),
            sipx_sip::Limits::stream(),
            &adopt,
            &runtime,
            admission,
            meters,
        )
        .await
        .expect("WSS listener binds");

        let first = TcpStream::connect(address).await.expect("first connects");
        wait_for_observation(&mut observed, HandshakeObservation::Admitted).await;
        wait_for_available(&runtime, 0).await;
        let refused = TcpStream::connect(address).await.expect("second connects");
        wait_for_observation(&mut observed, HandshakeObservation::Refused).await;
        wait_for_eof(&refused).await;

        wait_for_eof(&first).await;
        wait_for_available(&runtime, 1).await;

        let admitted = TcpStream::connect(address).await.expect("third connects");
        wait_for_observation(&mut observed, HandshakeObservation::Admitted).await;
        wait_for_available(&runtime, 0).await;
        owner.shutdown().await;
        wait_for_eof(&admitted).await;
    }
}
