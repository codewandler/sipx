//! The endpoint: one event loop driving the sans-IO core.
//!
//! Everything mutable lives in this loop — the transaction layer, the timer queue, the
//! sockets. No transaction is reachable from two tasks, so there are no locks in the
//! signalling path and no way to observe a half-applied transition. Applications talk to the
//! loop over channels.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use sipx_sip::transaction::{Dispatch, Output, TransactionKey, TransactionLayer, TuEvent};
use sipx_sip::{Header, HeaderName, Limits, Message, Request, Response, Timers, parse_datagram};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, Result};
use crate::nat::apply_received_and_rport;
use crate::target::{Target, TransportKind, response_destination};
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
            pool: PoolConfig::default(),
        }
    }
}

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
            if let TuEvent::Response(response) = event {
                if response.status.is_final() {
                    return Some(*response);
                }
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
    },
    Shutdown,
}

/// A handle to a running endpoint.
#[derive(Debug, Clone)]
pub struct Handle {
    commands: mpsc::Sender<Command>,
    local_addr: SocketAddr,
    sent_by: Arc<String>,
    sent_by_port: u16,
}

impl Handle {
    /// The address the endpoint is bound to.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
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
                "SIP/2.0/{} {}:{};rport;branch={}",
                target.transport.as_str(),
                self.sent_by,
                self.sent_by_port,
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

    /// Send a response on a server transaction.
    pub async fn respond(&self, key: &TransactionKey, response: Response) -> Result<()> {
        self.commands
            .send(Command::Respond {
                key: key.clone(),
                response: Box::new(response),
            })
            .await
            .map_err(|_| Error::EndpointClosed)
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

    let (commands_tx, commands_rx) = mpsc::channel(config.capacity);
    let (incoming_tx, incoming_rx) = mpsc::channel(config.capacity);

    let handle = Handle {
        commands: commands_tx,
        local_addr,
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
        clients: HashMap::new(),
        incoming: incoming_tx,
        commands: commands_rx,
        net: net_rx,
        accepts: accept_rx,
        pool: Pool::new(config.pool, config.limits, net_tx),
        limits: config.limits,
        mtu: config.mtu,
    };
    tokio::spawn(driver.run());

    Ok((handle, incoming_rx))
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
    timers: TimerQueue,
    destinations: HashMap<TransactionKey, Target>,
    clients: HashMap<TransactionKey, mpsc::Sender<TuEvent>>,
    incoming: mpsc::Sender<Incoming>,
    commands: mpsc::Receiver<Command>,
    net: mpsc::Receiver<tcp::Event>,
    accepts: mpsc::Receiver<(tokio::net::TcpStream, SocketAddr)>,
    pool: Pool,
    limits: Limits,
    mtu: usize,
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
                _ = idle_sweep.tick() => {
                    for peer in self.pool.evict_idle() {
                        tracing::debug!(%peer, "closed an idle connection");
                    }
                }
            }
        }
    }

    async fn on_datagram(&mut self, datagram: Bytes, source: SocketAddr) {
        match parse_datagram(datagram, &self.limits) {
            Ok(message) => self.on_message(message, source, TransportKind::Udp).await,
            Err(error) => {
                // One malformed packet must not disturb the socket. The alternative is a
                // trivial denial of service.
                tracing::debug!(%error, %source, "dropping malformed datagram");
            }
        }
    }

    async fn on_net_event(&mut self, event: tcp::Event) {
        match event {
            tcp::Event::Message { message, source } => {
                self.on_message(*message, source, TransportKind::Tcp).await;
            }
            tcp::Event::Closed { peer } => {
                self.pool.remove(peer);
                self.fail_transactions_on(peer, TransportKind::Tcp).await;
            }
        }
    }

    /// Fail every transaction bound to a connection that has gone.
    ///
    /// The alternative is letting them time out, which means waiting up to 32 seconds to
    /// discover something already known — a bad experience and a resource leak.
    async fn fail_transactions_on(&mut self, peer: SocketAddr, transport: TransportKind) {
        let affected: Vec<TransactionKey> = self
            .destinations
            .iter()
            .filter(|(_, target)| target.addr == peer && target.transport == transport)
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
        let reply_to = match &message {
            Message::Request(request) if transport == TransportKind::Udp => request
                .headers
                .typed::<sipx_sip::headers::Via>()
                .and_then(std::result::Result::ok)
                .map_or_else(
                    || Target::new(source, transport),
                    |via| response_destination(&via, source, transport),
                ),
            _ => Target::new(source, transport),
        };

        match self.layer.receive(message, transport.reliability()) {
            Dispatch::Created { key, outputs } => {
                self.destinations.insert(key.clone(), reply_to);
                self.perform(&key, outputs, Some((source, transport))).await;
            }
            Dispatch::Matched { key, outputs } => {
                self.perform(&key, outputs, Some((source, transport))).await;
            }
            Dispatch::Unmatched(message) => {
                tracing::debug!(%source, "message matched no transaction");
                // An unmatched ACK belongs to the application; anything else is noise it can
                // still choose to look at.
                if let Message::Request(request) = *message {
                    let Some(key) = TransactionKey::from_request(&request) else {
                        return;
                    };
                    let _ = self.incoming.try_send(Incoming {
                        key,
                        request,
                        source,
                        transport,
                    });
                }
            }
        }
    }

    async fn on_timers(&mut self) {
        let due = self.timers.take_due(tokio::time::Instant::now());
        for fired in due {
            let outputs = self.layer.on_timer(&fired.key, fired.timer);
            self.perform(&fired.key, outputs, None).await;
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
            Command::Respond { key, response } => {
                let outputs = self.layer.send_response(&key, *response);
                self.perform(&key, outputs, None).await;
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
    ) {
        for output in outputs {
            match output {
                Output::Send(message) => {
                    let target =
                        self.destinations.get(key).copied().or_else(|| {
                            origin.map(|(addr, transport)| Target::new(addr, transport))
                        });
                    let Some(target) = target else {
                        tracing::warn!("no destination for a message the transaction wants sent");
                        continue;
                    };
                    let is_response = matches!(*message, Message::Response(_));
                    let bytes = message.to_bytes();
                    if let Err(error) = self.transmit(bytes, target, is_response).await {
                        tracing::warn!(%error, addr = %target.addr, "send failed");
                        let outputs = self.layer.on_transport_error(key);
                        Box::pin(self.perform(key, outputs, origin)).await;
                        return;
                    }
                }
                Output::SetTimer { timer, after } => self.timers.set(key.clone(), timer, after),
                Output::ClearTimer(timer) => self.timers.clear(key, timer),
                Output::ToTu(event) => self.deliver(key, *event, origin).await,
                Output::Terminated(_) => {
                    self.timers.forget(key);
                    self.destinations.remove(key);
                    // Dropping the sender closes the application's response stream, which is
                    // how it learns the transaction is over.
                    self.clients.remove(key);
                }
            }
        }
    }

    /// Put bytes on the wire, opening a connection if the transport needs one.
    ///
    /// `is_response` decides whether an inbound connection may be used. A response goes back
    /// over the connection its request arrived on — RFC 5923, and the only thing that works
    /// when the peer is behind a NAT. An outbound *request* is different: reusing an inbound
    /// connection for one is how a peer that connected to you gets your traffic routed
    /// through it, so that is off unless configured.
    async fn transmit(&mut self, bytes: Bytes, target: Target, is_response: bool) -> Result<()> {
        match target.transport {
            TransportKind::Udp => {
                if bytes.len() > self.mtu {
                    // RFC 3261 §18.1.1. Refusing by name beats sending something that will be
                    // fragmented or silently truncated — a truncated SIP message is a security
                    // problem, not a degraded one.
                    return Err(Error::TooLarge {
                        size: bytes.len(),
                        mtu: self.mtu,
                    });
                }
                self.socket.send_to(&bytes, target.addr).await?;
                Ok(())
            }
            TransportKind::Tcp => {
                if is_response && self.pool.send_on_existing(target.addr, bytes.clone()).await {
                    return Ok(());
                }
                self.pool.send(target.addr, bytes).await
            }
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
                    // calls; dropping the event silently loses a request. 503 tells the peer
                    // something true.
                    tracing::warn!("application queue full; refusing the transaction");
                    self.refuse(key).await;
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
