//! The TCP transport: stream framing and a connection pool.
//!
//! A stream is not a sequence of messages until something makes it one, so each connection
//! owns a [`StreamParser`] and hands completed messages to the endpoint loop. Connections are
//! pooled and reused, because opening one per request is both slow and, for a peer behind a
//! NAT, impossible in the reverse direction.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::{Duration, Instant};

use bytes::Bytes;
use sipx_sip::{Limits, Message, StreamParser};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::target::{ConnectionKey, TransportKind};

/// Something that happened on a connection.
#[derive(Debug)]
pub enum Event {
    /// A complete message arrived.
    Message {
        /// The message.
        message: Box<Message>,
        /// Which peer sent it.
        source: SocketAddr,
        /// Which transport carried it. A TLS connection and a TCP one are both streams and
        /// share every line of framing below; only this distinguishes them, and a message that
        /// arrived over TLS must not be reported as cleartext.
        transport: TransportKind,
        /// Which incarnation delivered it.
        id: u64,
    },
    /// Framing was lost, so the connection is being closed and everything in flight with it.
    ///
    /// Reported rather than only logged because it is the stream half of a parse failure, and §12
    /// counts those *per transport*: "a malformed datagram and a stream whose framing is lost are
    /// the same failure on different transports". The connection task has no counter in scope — it
    /// is spawned before the driver — so it says what happened and the driver counts it, which is
    /// also how every other counter in this crate stays at one increment site.
    ///
    /// A `Closed` follows. This does not replace it: what is lost and what is closed are two facts,
    /// and an operator needs the first to explain the second.
    FramingFailed {
        /// Which connection lost framing.
        key: ConnectionKey,
    },
    /// A CRLF keep-alive arrived on this connection (RFC 5626 §4.4.1).
    ///
    /// Reported rather than dropped because it is the *pong* half of the mechanism: a UA that sent
    /// a CRLFCRLF ping "MUST treat the flow as failed" if no single-CRLF pong comes back within 10
    /// seconds, which it cannot do if nothing tells it one arrived.
    Pong {
        /// Which connection it arrived on.
        key: ConnectionKey,
        /// Which incarnation received it, so an old queued pong cannot answer a new flow.
        id: u64,
    },
    /// TLS authentication failed before the connection became usable.
    ///
    /// Separate from `Closed` so a caller can distinguish a rejected certificate from an
    /// established connection that later disappeared. `Closed` still follows and releases the
    /// pool slot.
    #[cfg(feature = "tls")]
    HandshakeFailed {
        /// Which secure connection was attempted.
        key: ConnectionKey,
        /// Which incarnation failed.
        id: u64,
        /// The TLS backend's verification detail, containing no key material.
        detail: String,
    },
    /// The connection is gone.
    ///
    /// Every transaction bound to it is given a transport error rather than being left to
    /// time out: waiting 32 seconds to learn something we already know is both a bad
    /// experience and a resource leak.
    Closed {
        /// Which connection closed. The whole key, not just the peer: two connections to one
        /// address are ordinary now, and removing the wrong one is worse than removing none.
        key: ConnectionKey,
        /// Which incarnation of the key closed, so a retiring task cannot remove its replacement.
        id: u64,
    },
}

/// How the pool is configured.
#[derive(Debug, Clone, Copy)]
pub struct PoolConfig {
    /// Most connections held at once.
    pub max_connections: usize,
    /// How long a connection may sit unused before it is closed.
    pub idle_timeout: Duration,
    /// Whether an inbound connection may carry unrelated outbound requests.
    ///
    /// Off by default, and that is the security-relevant decision recorded in
    /// `docs/specs/sip-transport.md` §8: reusing an inbound connection is convenient and is
    /// also how a peer that connected to you gets your outbound traffic routed through it.
    pub reuse_inbound_for_outbound: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_connections: 1024,
            idle_timeout: Duration::from_secs(300),
            reuse_inbound_for_outbound: false,
        }
    }
}

/// How a connection came to exist, which decides whether it may be reused for outbound
/// requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// We opened it.
    Outbound,
    /// A peer opened it.
    Inbound,
}

#[derive(Debug)]
struct Pooled {
    writer: mpsc::Sender<Bytes>,
    cancel: CancellationToken,
    id: u64,
    origin: Origin,
    last_used: Instant,
    active: bool,
}

type ConnectionTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

struct Pending {
    key: ConnectionKey,
    id: u64,
    cancel: CancellationToken,
    task: ConnectionTask,
}

struct Registration {
    writer: mpsc::Receiver<Bytes>,
    cancel: CancellationToken,
    id: u64,
    start_now: bool,
}

/// A pool of stream connections.
pub struct Pool {
    connections: HashMap<ConnectionKey, Pooled>,
    config: PoolConfig,
    events: mpsc::Sender<Event>,
    limits: Limits,
    tasks: JoinSet<()>,
    next_id: u64,
    shutdown: CancellationToken,
    /// Generations deliberately retired but still entitled to one final close notification.
    retiring: HashSet<(ConnectionKey, u64)>,
    /// Replacement work that owns a logical pool slot but cannot start until its victim exits.
    pending: VecDeque<Pending>,
}

impl std::fmt::Debug for Pool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Pool")
            .field("connections", &self.connections)
            .field("config", &self.config)
            .field("tasks", &self.tasks.len())
            .field("retiring", &self.retiring.len())
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

impl Pool {
    /// A pool that reports what it receives to `events`.
    #[must_use]
    pub fn new(config: PoolConfig, limits: Limits, events: mpsc::Sender<Event>) -> Self {
        Self {
            connections: HashMap::new(),
            config,
            events,
            limits,
            tasks: JoinSet::new(),
            next_id: 1,
            shutdown: CancellationToken::new(),
            retiring: HashSet::new(),
            pending: VecDeque::new(),
        }
    }

    /// How many connections are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// Whether the pool is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Adopt a connection a peer opened.
    pub fn accept(&mut self, stream: TcpStream, peer: SocketAddr) {
        let key = ConnectionKey::new(peer, TransportKind::Tcp);
        if self.insert_stream(stream, key, Origin::Inbound).is_err() {
            // discard: a retiring task still owns the last live slot, so admitting this socket
            // would exceed the configured bound. The peer may retry after termination.
            tracing::debug!(%peer, "refused inbound TCP connection at capacity");
        }
    }

    /// Send to a peer, connecting if there is no usable connection.
    ///
    /// The dial never blocks the caller. A peer that black-holes SYN takes the OS connect
    /// timeout to fail — around two minutes — and the endpoint loop that calls this also owns
    /// every transaction timer. Waiting here would stop retransmissions for calls that have
    /// nothing to do with this peer, so the connection is established inside its own task and
    /// the bytes wait in the channel until it is up.
    pub async fn send(&mut self, key: &ConnectionKey, bytes: Bytes) -> Result<()> {
        self.send_generation(key, bytes).await.map(|_| ())
    }

    pub(crate) async fn send_generation(
        &mut self,
        key: &ConnectionKey,
        bytes: Bytes,
    ) -> Result<u64> {
        if !self.reusable(key) {
            // Either there is nothing here, the writer is gone, or policy forbids reusing an
            // inbound connection for our own requests. All three mean: open our own.
            self.dial(key.clone())?;
        }
        self.queue(key, bytes).await?;
        self.generation(key)
            .ok_or(crate::error::Error::EndpointClosed)
    }

    /// Adopt a TLS connection a peer opened, once the handshake has completed.
    #[cfg(feature = "tls")]
    pub fn accept_tls(
        &mut self,
        stream: tokio_rustls::server::TlsStream<TcpStream>,
        peer: SocketAddr,
    ) {
        let key = ConnectionKey::new(peer, TransportKind::Tls);
        if self
            .insert_stream(stream, key.clone(), Origin::Inbound)
            .is_err()
        {
            // discard: a retiring task still owns the last live slot, so admitting this socket
            // would exceed the configured bound. The peer may retry after termination.
            tracing::debug!(peer = %key.peer, "refused inbound TLS connection at capacity");
        }
    }

    /// Send to a peer over TLS, connecting and verifying if there is no usable connection.
    ///
    /// The verification name is the host from the URI, not the address — see
    /// `docs/specs/sip-tls.md` §3.3 — and it is part of the pool key, so a connection verified
    /// as one name is never handed to traffic for another.
    #[cfg(feature = "tls")]
    pub async fn send_tls(
        &mut self,
        key: &ConnectionKey,
        verification_name: &str,
        client: &crate::tls::ClientTls,
        bytes: Bytes,
    ) -> Result<()> {
        self.send_tls_generation(key, verification_name, client, bytes)
            .await
            .map(|_| ())
    }

    #[cfg(feature = "tls")]
    pub(crate) async fn send_tls_generation(
        &mut self,
        key: &ConnectionKey,
        verification_name: &str,
        client: &crate::tls::ClientTls,
        bytes: Bytes,
    ) -> Result<u64> {
        if !self.reusable(key) {
            self.dial_tls(key.clone(), verification_name, client)?;
        }
        self.queue(key, bytes).await?;
        self.generation(key)
            .ok_or(crate::error::Error::EndpointClosed)
    }

    /// Adopt a WebSocket connection a peer opened, once the handshake has completed.
    #[cfg(feature = "ws")]
    pub fn accept_ws<S>(
        &mut self,
        ws: crate::ws::Socket<S>,
        key: ConnectionKey,
        keepalive: Duration,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let peer = key.peer;
        if self.spawn_ws(ws, key, Origin::Inbound, keepalive).is_err() {
            // discard: a retiring task still owns the last live slot, so admitting this socket
            // would exceed the configured bound. The peer may retry after termination.
            tracing::debug!(%peer, "refused inbound WebSocket connection at capacity");
        }
    }

    /// Send to a peer over WebSocket, doing the handshake if there is no usable connection.
    ///
    /// `authority` is what goes in the `Host` header of the upgrade request and, under WSS, the
    /// name the certificate must be valid for. Both are the host from the URI.
    #[cfg(feature = "ws")]
    pub async fn send_ws(
        &mut self,
        key: &ConnectionKey,
        authority: &str,
        keepalive: Duration,
        #[cfg(feature = "wss")] client: Option<&crate::tls::ClientTls>,
        bytes: Bytes,
    ) -> Result<()> {
        self.send_ws_generation(
            key,
            authority,
            keepalive,
            #[cfg(feature = "wss")]
            client,
            bytes,
        )
        .await
        .map(|_| ())
    }

    #[cfg(feature = "ws")]
    pub(crate) async fn send_ws_generation(
        &mut self,
        key: &ConnectionKey,
        authority: &str,
        keepalive: Duration,
        #[cfg(feature = "wss")] client: Option<&crate::tls::ClientTls>,
        bytes: Bytes,
    ) -> Result<u64> {
        if !self.reusable(key) {
            self.dial_ws(
                key.clone(),
                authority,
                keepalive,
                #[cfg(feature = "wss")]
                client,
            )?;
        }
        self.queue(key, bytes).await?;
        self.generation(key)
            .ok_or(crate::error::Error::EndpointClosed)
    }

    /// Forget a connection that has closed.
    pub fn remove(&mut self, key: &ConnectionKey, id: u64) -> bool {
        let removed = if self
            .connections
            .get(key)
            .is_some_and(|pooled| pooled.id == id)
        {
            self.connections.remove(key);
            true
        } else {
            self.retiring.remove(&(key.clone(), id))
        };
        self.reap_finished();
        self.activate_pending();
        removed
    }

    /// The generation currently routing traffic for this key, including a reserved replacement.
    #[must_use]
    pub(crate) fn generation(&self, key: &ConnectionKey) -> Option<u64> {
        self.connections.get(key).map(|pooled| pooled.id)
    }

    /// Whether this connection is held.
    #[must_use]
    pub fn holds(&self, key: &ConnectionKey) -> bool {
        self.connections.contains_key(key)
    }

    /// Answer on the connection a request arrived over, if it is still open.
    ///
    /// Always tried before anything the `Via` says: opening a new connection to a NAT-ed
    /// client's advertised address cannot work, which is what RFC 5923 exists to say.
    ///
    /// [`Via`]: sipx_sip::headers::Via
    pub async fn send_on_existing(&mut self, key: &ConnectionKey, bytes: Bytes) -> bool {
        self.send_on_existing_generation(key, bytes).await.is_some()
    }

    pub(crate) async fn send_on_existing_generation(
        &mut self,
        key: &ConnectionKey,
        bytes: Bytes,
    ) -> Option<u64> {
        let pooled = self.connections.get_mut(key)?;
        if pooled.writer.send(bytes).await.is_err() {
            self.retire(key);
            return None;
        }
        pooled.last_used = Instant::now();
        Some(pooled.id)
    }

    /// Close connections idle for longer than the configured timeout.
    pub fn evict_idle(&mut self) -> Vec<ConnectionKey> {
        let deadline = Instant::now();
        let idle_timeout = self.config.idle_timeout;
        let evicted: Vec<ConnectionKey> = self
            .connections
            .iter()
            .filter(|(_, c)| deadline.duration_since(c.last_used) > idle_timeout)
            .map(|(key, _)| key.clone())
            .collect();
        for key in &evicted {
            self.retire(key);
        }
        evicted
    }

    /// Whether the connection already held for this key may carry what is about to be sent.
    fn reusable(&self, key: &ConnectionKey) -> bool {
        self.connections.get(key).is_some_and(|pooled| {
            !pooled.writer.is_closed()
                && (pooled.origin == Origin::Outbound || self.config.reuse_inbound_for_outbound)
        })
    }

    /// Hand bytes to a connection's writer.
    async fn queue(&mut self, key: &ConnectionKey, bytes: Bytes) -> Result<()> {
        let Some(pooled) = self.connections.get_mut(key) else {
            return Err(crate::error::Error::EndpointClosed);
        };
        pooled.last_used = Instant::now();
        let writer = pooled.writer.clone();
        writer
            .send(bytes)
            .await
            .map_err(|_| crate::error::Error::EndpointClosed)
    }

    /// Reserve a live-task slot and take the writer half of a connection about to be spawned.
    ///
    /// The writer exists before the socket does, so bytes queue in the channel rather than in
    /// the caller while a connection is still being established.
    fn register(&mut self, key: ConnectionKey, origin: Origin) -> Result<Registration> {
        self.reap_finished();
        self.activate_pending();
        // One admission may retire one connection. Replacing this exact key retires that
        // generation; a new key at capacity retires the LRU. Doing both would evict unrelated
        // connection B merely because replacement A still occupies its slot while cancelling.
        if self.connections.contains_key(&key) {
            self.retire(&key);
        } else if self.connections.len() >= self.config.max_connections {
            self.evict_least_recently_used();
        }
        self.reap_finished();
        self.activate_pending();
        if self.connections.len() >= self.config.max_connections {
            return Err(crate::error::Error::ConnectionCapacity {
                max: self.config.max_connections,
            });
        }
        let (writer_tx, writer_rx) = mpsc::channel::<Bytes>(64);
        let cancel = CancellationToken::new();
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.connections.insert(
            key,
            Pooled {
                writer: writer_tx,
                cancel: cancel.clone(),
                id,
                origin,
                last_used: Instant::now(),
                active: self.tasks.len() < self.config.max_connections,
            },
        );
        Ok(Registration {
            writer: writer_rx,
            cancel,
            id,
            start_now: self.tasks.len() < self.config.max_connections,
        })
    }

    fn launch<F>(
        &mut self,
        key: ConnectionKey,
        id: u64,
        cancel: CancellationToken,
        start_now: bool,
        task: F,
    ) where
        F: Future<Output = ()> + Send + 'static,
    {
        if start_now {
            self.track(key, id, cancel, task);
        } else {
            self.pending.push_back(Pending {
                key,
                id,
                cancel,
                task: Box::pin(task),
            });
        }
    }

    fn activate_pending(&mut self) {
        while self.tasks.len() < self.config.max_connections {
            let Some(pending) = self.pending.pop_front() else {
                break;
            };
            let is_current = self
                .connections
                .get_mut(&pending.key)
                .filter(|pooled| pooled.id == pending.id);
            let Some(pooled) = is_current else {
                continue;
            };
            pooled.active = true;
            self.track(pending.key, pending.id, pending.cancel, pending.task);
        }
    }

    fn track<F>(&mut self, key: ConnectionKey, id: u64, cancel: CancellationToken, task: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let events = self.events.clone();
        let shutdown = self.shutdown.clone();
        self.tasks.spawn(async move {
            let report = tokio::select! {
                biased;
                () = shutdown.cancelled() => false,
                () = cancel.cancelled() => true,
                () = task => true,
            };
            if report {
                // discard: the endpoint driver may already be gone. The tracked task still ends
                // and releases its live slot, which is the resource guarantee this path owns.
                tokio::select! {
                    biased;
                    () = shutdown.cancelled() => {}
                    result = events.send(Event::Closed { key, id }) => {
                        let _ = result;
                    }
                }
            }
        });
    }

    fn dial(&mut self, key: ConnectionKey) -> Result<()> {
        let Registration {
            writer: writer_rx,
            cancel,
            id,
            start_now,
        } = self.register(key.clone(), Origin::Outbound)?;
        let events = self.events.clone();
        let limits = self.limits;
        let task_key = key.clone();
        self.launch(key, id, cancel, start_now, async move {
            match TcpStream::connect(task_key.peer).await {
                Ok(stream) => pump(stream, task_key, id, writer_rx, events, limits).await,
                Err(error) => {
                    tracing::debug!(%error, peer = %task_key.peer, "connect failed");
                }
            }
        });
        Ok(())
    }

    #[cfg(feature = "tls")]
    fn dial_tls(
        &mut self,
        key: ConnectionKey,
        verification_name: &str,
        client: &crate::tls::ClientTls,
    ) -> Result<()> {
        let name = crate::tls::verification_name(verification_name)?;
        let connector = client.connector();
        let Registration {
            writer: writer_rx,
            cancel,
            id,
            start_now,
        } = self.register(key.clone(), Origin::Outbound)?;
        let events = self.events.clone();
        let limits = self.limits;
        let task_key = key.clone();

        self.launch(key, id, cancel, start_now, async move {
            let stream = match TcpStream::connect(task_key.peer).await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::debug!(%error, peer = %task_key.peer, "connect failed");
                    return;
                }
            };
            match connector.connect(name, stream).await {
                Ok(tls) => pump(tls, task_key, id, writer_rx, events, limits).await,
                Err(error) => {
                    // Every verification failure arrives here, with the reason attached. It is
                    // logged rather than swallowed, and the connection simply does not exist —
                    // there is no fallback to cleartext.
                    tracing::warn!(%error, peer = %task_key.peer, "TLS handshake failed");
                    // discard: the endpoint may have shut down before this bounded task reports;
                    // no caller remains to receive the typed failure in that case.
                    let _ = events
                        .send(Event::HandshakeFailed {
                            key: task_key,
                            id,
                            detail: error.to_string(),
                        })
                        .await;
                }
            }
        });
        Ok(())
    }

    #[cfg(feature = "ws")]
    fn dial_ws(
        &mut self,
        key: ConnectionKey,
        authority: &str,
        keepalive: Duration,
        #[cfg(feature = "wss")] client: Option<&crate::tls::ClientTls>,
    ) -> Result<()> {
        #[cfg(feature = "wss")]
        let secure = if key.transport == TransportKind::Wss {
            let client = client.ok_or(crate::error::Error::UnsupportedTransport(
                "WSS (no client configuration, so no outbound connection can be verified)",
            ))?;
            // The name a certificate is checked against, which is *not* `authority`: that
            // carries a port because an HTTP `Host` header does, and a certificate is issued to
            // a host. The identity on the key is the bare host from the URI — see
            // `docs/specs/sip-tls.md` §3.3 — with the address as the fallback, exactly as the
            // TLS transport does it.
            let verify = key
                .identity
                .as_deref()
                .map_or_else(|| key.peer.ip().to_string(), str::to_owned);
            Some((client.connector(), crate::tls::verification_name(&verify)?))
        } else {
            None
        };

        let authority = authority.to_owned();
        let Registration {
            writer: writer_rx,
            cancel,
            id,
            start_now,
        } = self.register(key.clone(), Origin::Outbound)?;
        let events = self.events.clone();
        let limits = self.limits;
        let task_key = key.clone();

        self.launch(key, id, cancel, start_now, async move {
            let stream = match TcpStream::connect(task_key.peer).await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::debug!(%error, peer = %task_key.peer, "connect failed");
                    return;
                }
            };

            #[cfg(feature = "wss")]
            if let Some((connector, name)) = secure {
                match connector.connect(name, stream).await {
                    Ok(tls) => {
                        crate::ws::dial(
                            tls, &authority, task_key, id, writer_rx, events, limits, keepalive,
                        )
                        .await;
                    }
                    Err(error) => {
                        tracing::warn!(%error, peer = %task_key.peer, "TLS handshake failed");
                        // discard: the endpoint may have shut down before this bounded task
                        // reports; no caller remains to receive the typed failure in that case.
                        let _ = events
                            .send(Event::HandshakeFailed {
                                key: task_key,
                                id,
                                detail: error.to_string(),
                            })
                            .await;
                    }
                }
                return;
            }

            crate::ws::dial(
                stream, &authority, task_key, id, writer_rx, events, limits, keepalive,
            )
            .await;
        });
        Ok(())
    }

    fn insert_stream<S>(&mut self, stream: S, key: ConnectionKey, origin: Origin) -> Result<()>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
    {
        let Registration {
            writer: writer_rx,
            cancel,
            id,
            start_now,
        } = self.register(key.clone(), origin)?;
        let events = self.events.clone();
        let limits = self.limits;
        let task_key = key.clone();
        self.launch(
            key,
            id,
            cancel,
            start_now,
            pump(stream, task_key, id, writer_rx, events, limits),
        );
        Ok(())
    }

    #[cfg(feature = "ws")]
    fn spawn_ws<S>(
        &mut self,
        ws: crate::ws::Socket<S>,
        key: ConnectionKey,
        origin: Origin,
        keepalive: Duration,
    ) -> Result<()>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let Registration {
            writer: writer_rx,
            cancel,
            id,
            start_now,
        } = self.register(key.clone(), origin)?;
        let events = self.events.clone();
        let limits = self.limits;
        let task_key = key.clone();
        self.launch(
            key,
            id,
            cancel,
            start_now,
            crate::ws::pump(ws, task_key, id, writer_rx, events, limits, keepalive),
        );
        Ok(())
    }

    fn retire(&mut self, key: &ConnectionKey) {
        if let Some(pooled) = self.connections.remove(key) {
            pooled.cancel.cancel();
            if pooled.active {
                self.retiring.insert((key.clone(), pooled.id));
            } else if let Some(index) = self
                .pending
                .iter()
                .position(|pending| pending.key == *key && pending.id == pooled.id)
            {
                self.pending.remove(index);
            }
        }
    }

    fn reap_finished(&mut self) {
        while self.tasks.try_join_next().is_some() {}
    }

    /// Cancel every connection and wait until every tracked task has released its socket.
    pub async fn shutdown(&mut self) {
        self.shutdown.cancel();
        for pooled in self.connections.values() {
            pooled.cancel.cancel();
        }
        self.connections.clear();
        self.pending.clear();
        self.retiring.clear();
        while self.tasks.join_next().await.is_some() {}
    }

    fn evict_least_recently_used(&mut self) {
        let Some(victim) = self
            .connections
            .iter()
            .min_by_key(|(_, c)| c.last_used)
            .map(|(key, _)| key.clone())
        else {
            return;
        };
        self.retire(&victim);
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shutdown.cancel();
        for pooled in self.connections.values() {
            pooled.cancel.cancel();
        }
        self.pending.clear();
        // Dropping the JoinSet aborts any task that has not observed cancellation yet. This is
        // the synchronous fallback for a runtime teardown that cannot await `Pool::shutdown`.
    }
}

/// Read and write one connection until it ends.
///
/// Generic over the stream so a TLS connection reuses this wholesale: a TLS connection differs
/// from a TCP one in its bytes, not in its framing or its transaction handling, and a second
/// copy of this loop is a second place for the framing rules to drift.
async fn pump<S>(
    stream: S,
    key: ConnectionKey,
    id: u64,
    mut outgoing: mpsc::Receiver<Bytes>,
    events: mpsc::Sender<Event>,
    limits: Limits,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
{
    let (peer, transport) = (key.peer, key.transport);
    let (mut read_half, mut write_half) = tokio::io::split(stream);
    let mut parser = StreamParser::new(limits);
    let mut buf = vec![0u8; 8192];

    loop {
        tokio::select! {
            read = read_half.read(&mut buf) => match read {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let chunk = buf.get(..n).unwrap_or(&[]);
                    match parser.push(chunk) {
                        Ok(messages) => {
                            // Before the messages: a pong can arrive in the same chunk as an
                            // unrelated request, and the flow is alive either way.
                            for _ in 0..parser.take_keepalives() {
                                if events
                                    .send(Event::Pong {
                                        key: key.clone(),
                                        id,
                                    })
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            for message in messages {
                                if events
                                    .send(Event::Message {
                                        message: Box::new(message),
                                        source: peer,
                                        transport,
                                        id,
                                    })
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }
                        Err(error) => {
                            // Framing is lost. Resynchronizing would mean guessing where the
                            // next message starts, which is how a body becomes a request.
                            //
                            // discard: everything in flight on this connection, which is the
                            // largest single loss in this file. Counted, but not here — the
                            // `FramingFailed` below carries it to the driver, which owns every
                            // counter in this crate (§12.1). A connection task is spawned before the
                            // driver exists and has no `Meters` in scope.
                            tracing::debug!(%error, %peer, "closing connection on framing error");
                            // discard: the driver has stopped, so there is no longer anyone to tell
                            // that framing was lost. The `Closed` below is discarded for the same
                            // reason and the connection closes as it drops.
                            let _ = events.send(Event::FramingFailed { key: key.clone() }).await;
                            break;
                        }
                    }
                }
            },
            Some(bytes) = outgoing.recv() => {
                if write_half.write_all(&bytes).await.is_err() {
                    break;
                }
            }
            else => break,
        }
    }
}

/// Accept connections on a listener, adding each to the pool.
///
/// Returns when the listener fails.
pub async fn accept_loop(listener: TcpListener, incoming: mpsc::Sender<(TcpStream, SocketAddr)>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                if incoming.send((stream, peer)).await.is_err() {
                    return;
                }
            }
            Err(error) => {
                tracing::warn!(%error, "accept failed");
                return;
            }
        }
    }
}

/// Accept connections until endpoint cancellation or listener failure.
pub(crate) async fn accept_loop_until(
    listener: TcpListener,
    incoming: mpsc::Sender<(TcpStream, SocketAddr)>,
    cancel: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            biased;
            () = cancel.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, peer)) => {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return,
                    result = incoming.send((stream, peer)) => {
                        if result.is_err() {
                            return;
                        }
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, "accept failed");
                return;
            }
        }
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

    const MESSAGE: &str = "OPTIONS sip:a@b.com SIP/2.0\r\n\
         Via: SIP/2.0/TCP h.example.com;branch=z9hG4bKx\r\n\
         To: <sip:a@b.com>\r\n\
         From: <sip:c@d.net>;tag=1\r\n\
         Call-ID: x@y\r\n\
         CSeq: 1 OPTIONS\r\n\
         Content-Length: 0\r\n\r\n";

    async fn pool_with_listener() -> (Pool, SocketAddr, mpsc::Receiver<Event>, TcpListener) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let addr = listener.local_addr().expect("has an address");
        let (tx, rx) = mpsc::channel(64);
        let pool = Pool::new(PoolConfig::default(), Limits::stream(), tx);
        (pool, addr, rx, listener)
    }

    fn tcp(peer: SocketAddr) -> ConnectionKey {
        ConnectionKey::new(peer, TransportKind::Tcp)
    }

    async fn wait_until_no_live_tasks(pool: &mut Pool) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                pool.reap_finished();
                if pool.is_empty() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("connection tasks terminate");
    }

    /// X5: the property a stream transport lives or dies by.
    #[tokio::test]
    async fn tcp_message_split_across_segments_is_assembled() {
        let (mut pool, addr, mut events, listener) = pool_with_listener().await;

        let accepted = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.expect("accepts");
            (stream, peer)
        });

        let mut client = TcpStream::connect(addr).await.expect("connects");
        let (stream, peer) = accepted.await.expect("accepted");
        pool.accept(stream, peer);

        // Write the message one byte at a time, which is the worst case a real network can
        // produce and the one hand-written framers get wrong.
        for byte in MESSAGE.as_bytes() {
            client.write_all(&[*byte]).await.expect("writes");
            client.flush().await.expect("flushes");
        }

        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("no timeout")
            .expect("an event");
        match event {
            Event::Message { message, .. } => {
                assert_eq!(message.to_bytes().as_ref(), MESSAGE.as_bytes());
            }
            Event::Pong { .. }
            | Event::Closed { .. }
            | Event::FramingFailed { .. }
            | Event::HandshakeFailed { .. } => {
                panic!("expected a message")
            }
        }
    }

    /// X6: two messages in one write must both come out, in order.
    #[tokio::test]
    async fn two_messages_in_one_segment_are_both_delivered() {
        let (mut pool, addr, mut events, listener) = pool_with_listener().await;
        let accepted = tokio::spawn(async move { listener.accept().await.expect("accepts") });
        let mut client = TcpStream::connect(addr).await.expect("connects");
        let (stream, peer) = accepted.await.expect("accepted");
        pool.accept(stream, peer);

        let both = format!("{MESSAGE}{MESSAGE}");
        client.write_all(both.as_bytes()).await.expect("writes");

        for _ in 0..2 {
            let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("no timeout")
                .expect("an event");
            assert!(matches!(event, Event::Message { .. }));
        }
    }

    /// X7: a closed connection is reported at once, so the transactions bound to it fail now
    /// rather than in 32 seconds.
    #[tokio::test]
    async fn a_closed_connection_is_reported_immediately() {
        let (mut pool, addr, mut events, listener) = pool_with_listener().await;
        let accepted = tokio::spawn(async move { listener.accept().await.expect("accepts") });
        let client = TcpStream::connect(addr).await.expect("connects");
        let (stream, peer) = accepted.await.expect("accepted");
        pool.accept(stream, peer);

        drop(client);

        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("no timeout")
            .expect("an event");
        assert!(matches!(event, Event::Closed { .. }));
    }

    /// Framing errors are terminal for the connection, because guessing where the next
    /// message begins is how a body becomes a request.
    #[tokio::test]
    async fn a_framing_error_closes_the_connection() {
        let (mut pool, addr, mut events, listener) = pool_with_listener().await;
        let accepted = tokio::spawn(async move { listener.accept().await.expect("accepts") });
        let mut client = TcpStream::connect(addr).await.expect("connects");
        let (stream, peer) = accepted.await.expect("accepted");
        pool.accept(stream, peer);

        client
            .write_all(b"OPTIONS sip:a@b SIP/2.0\r\nContent-Length: -1\r\n\r\n")
            .await
            .expect("writes");

        // The loss is reported before the close, and both are needed: `FramingFailed` is what the
        // driver counts as a parse failure on this transport (§12), and `Closed` is what fails the
        // transactions bound to the connection. Reporting only the second would leave "everything in
        // flight on this connection is gone" as a `tracing::debug!` and nothing else, which is what
        // it was.
        let first = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("no timeout")
            .expect("an event");
        assert!(
            matches!(first, Event::FramingFailed { .. }),
            "a framing error must report the loss, not only the close: {first:?}"
        );

        let second = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("no timeout")
            .expect("an event");
        assert!(matches!(second, Event::Closed { .. }), "{second:?}");
    }

    /// The default refuses to carry unrelated outbound traffic over a connection a peer
    /// opened. Answering *on* that connection is a different thing and is always allowed.
    #[tokio::test]
    async fn an_inbound_connection_is_not_reused_for_outbound_requests_by_default() {
        let (mut pool, addr, _events, listener) = pool_with_listener().await;
        let accepted = tokio::spawn(async move { listener.accept().await.expect("accepts") });
        let _client = TcpStream::connect(addr).await.expect("connects");
        let (stream, peer) = accepted.await.expect("accepted");
        pool.accept(stream, peer);
        assert_eq!(pool.len(), 1);

        // Answering on it is fine.
        assert!(
            pool.send_on_existing(&tcp(peer), Bytes::from_static(b"x"))
                .await,
            "a response must go back over the connection it came in on"
        );

        // But an outbound request to that same peer must not ride it. With reuse off, the
        // pool drops the inbound entry and dials afresh; the dial fails here because the peer
        // is a client socket with nothing listening, which is exactly the point — it did not
        // silently reuse.
        let before = pool.len();
        let _ = pool.send(&tcp(peer), Bytes::from_static(b"y")).await;
        assert!(
            pool.connections.len() <= before,
            "the inbound connection must not have been adopted for outbound use"
        );
    }

    #[tokio::test]
    async fn idle_connections_are_evicted() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let addr = listener.local_addr().expect("has an address");
        let (tx, mut rx) = mpsc::channel(64);
        let mut pool = Pool::new(
            PoolConfig {
                idle_timeout: Duration::ZERO,
                ..PoolConfig::default()
            },
            Limits::stream(),
            tx,
        );

        let accepted = tokio::spawn(async move { listener.accept().await.expect("accepts") });
        let mut client = TcpStream::connect(addr).await.expect("connects");
        let (stream, peer) = accepted.await.expect("accepted");
        pool.accept(stream, peer);
        assert_eq!(pool.len(), 1);

        // A definition of silence, with the window set to zero by the pool's own configuration:
        // `idle_timeout: Duration::ZERO` means any hole at all counts as idle, and this is that
        // hole. Load lengthens it, which is the direction that keeps the connection idle (`X-44`).
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(pool.evict_idle().len(), 1);
        let mut byte = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(2), client.read(&mut byte))
            .await
            .expect("eviction closes promptly")
            .expect("read reports EOF");
        assert_eq!(read, 0, "the evicted peer must observe EOF");
        let closed = rx.recv().await.expect("connection reports completion");
        let Event::Closed { key, id } = closed else {
            panic!("expected a close event, got {closed:?}");
        };
        pool.remove(&key, id);
        wait_until_no_live_tasks(&mut pool).await;
        assert!(pool.is_empty(), "the live task releases its pool slot");
    }

    #[tokio::test]
    async fn capacity_eviction_closes_the_socket_before_reusing_its_slot() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let addr = listener.local_addr().expect("has an address");
        let (tx, mut events) = mpsc::channel(64);
        let mut pool = Pool::new(
            PoolConfig {
                max_connections: 1,
                ..PoolConfig::default()
            },
            Limits::stream(),
            tx,
        );

        let mut first = TcpStream::connect(addr).await.expect("first connects");
        let (first_server, first_peer) = listener.accept().await.expect("first accepts");
        pool.accept(first_server, first_peer);

        let mut replacement = TcpStream::connect(addr).await.expect("second connects");
        let (second_server, second_peer) = listener.accept().await.expect("second accepts");
        pool.accept(second_server, second_peer);
        assert_eq!(pool.len(), 1, "a retiring task still owns the sole slot");

        let mut byte = [0u8; 1];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), first.read(&mut byte))
                .await
                .expect("eviction closes promptly")
                .expect("first read completes"),
            0
        );
        let Event::Closed { key, id } = events.recv().await.expect("close reported") else {
            panic!("expected close");
        };
        assert!(pool.remove(&key, id));
        assert_eq!(
            pool.len(),
            1,
            "the reserved replacement takes the released slot"
        );
        assert!(
            pool.send_on_existing(&tcp(second_peer), Bytes::from_static(b"r"))
                .await
        );
        assert_eq!(
            replacement
                .read(&mut byte)
                .await
                .expect("replacement reads"),
            1
        );
        pool.shutdown().await;
    }

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn idle_websocket_eviction_closes_the_peer_and_finishes_the_task() {
        use futures_util::StreamExt as _;
        use tokio_tungstenite::tungstenite::protocol::Role;

        let (server_io, client_io) = tokio::io::duplex(1024);
        let server = crate::ws::Socket::from_raw_socket(server_io, Role::Server, None).await;
        let mut client = crate::ws::Socket::from_raw_socket(client_io, Role::Client, None).await;
        let (tx, mut events) = mpsc::channel(64);
        let mut pool = Pool::new(
            PoolConfig {
                idle_timeout: Duration::ZERO,
                ..PoolConfig::default()
            },
            Limits::stream(),
            tx,
        );
        let key = ConnectionKey::new(
            "127.0.0.1:5090".parse().expect("address"),
            TransportKind::Ws,
        );
        pool.accept_ws(server, key, Duration::from_secs(60));
        // A definition of silence, as above: `idle_timeout: Duration::ZERO` makes any hole an
        // idle one, and load lengthening this hole keeps it idle rather than ending it (`X-44`).
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(pool.evict_idle().len(), 1);

        let peer_end = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .expect("WebSocket eviction closes promptly");
        assert!(
            peer_end.is_none()
                || matches!(
                    peer_end,
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)))
                )
                || matches!(peer_end, Some(Err(_)))
        );
        let Event::Closed { key, id } = events.recv().await.expect("close reported") else {
            panic!("expected close");
        };
        pool.remove(&key, id);
        wait_until_no_live_tasks(&mut pool).await;
        assert!(pool.is_empty());
    }

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn capacity_eviction_closes_websocket_peers_without_exceeding_the_live_limit() {
        use futures_util::StreamExt as _;
        use tokio_tungstenite::tungstenite::protocol::Role;

        let (first_server_io, first_client_io) = tokio::io::duplex(1024);
        let first_server =
            crate::ws::Socket::from_raw_socket(first_server_io, Role::Server, None).await;
        let mut first_client =
            crate::ws::Socket::from_raw_socket(first_client_io, Role::Client, None).await;
        let (second_server_io, second_client_io) = tokio::io::duplex(1024);
        let second_server =
            crate::ws::Socket::from_raw_socket(second_server_io, Role::Server, None).await;
        let mut second_client =
            crate::ws::Socket::from_raw_socket(second_client_io, Role::Client, None).await;
        let (tx, mut events) = mpsc::channel(64);
        let mut pool = Pool::new(
            PoolConfig {
                max_connections: 1,
                ..PoolConfig::default()
            },
            Limits::stream(),
            tx,
        );
        let first_key = ConnectionKey::new(
            "127.0.0.1:5091".parse().expect("address"),
            TransportKind::Ws,
        );
        let second_key = ConnectionKey::new(
            "127.0.0.1:5092".parse().expect("address"),
            TransportKind::Ws,
        );
        pool.accept_ws(first_server, first_key, Duration::from_secs(60));
        pool.accept_ws(second_server, second_key, Duration::from_secs(60));
        assert_eq!(pool.len(), 1, "the retiring WebSocket still owns the slot");

        let first_end = tokio::time::timeout(Duration::from_secs(2), first_client.next())
            .await
            .expect("eviction closes the first WebSocket");
        assert!(
            first_end.is_none()
                || matches!(&first_end, Some(Err(_)))
                || matches!(
                    &first_end,
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)))
                )
        );
        let Event::Closed { key, id } = events.recv().await.expect("close reported") else {
            panic!("expected close");
        };
        assert!(pool.remove(&key, id));
        assert_eq!(pool.len(), 1, "the WebSocket reservation activates");
        assert!(
            tokio::time::timeout(Duration::from_millis(20), second_client.next())
                .await
                .is_err(),
            "the replacement remains live rather than being refused"
        );
        pool.shutdown().await;
    }

    #[tokio::test]
    async fn quiet_connection_churn_never_exceeds_the_live_task_limit() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let addr = listener.local_addr().expect("address");
        let (tx, mut events) = mpsc::channel(64);
        let mut pool = Pool::new(
            PoolConfig {
                max_connections: 2,
                ..PoolConfig::default()
            },
            Limits::stream(),
            tx,
        );

        for _ in 0..24 {
            let client = TcpStream::connect(addr).await.expect("connects");
            let (server, peer) = listener.accept().await.expect("accepts");
            pool.accept(server, peer);
            assert!(pool.len() <= 2, "live tasks exceeded the configured limit");
            if !pool.holds(&tcp(peer)) {
                drop(client);
                if let Some(Event::Closed { key, id }) = events.recv().await {
                    pool.remove(&key, id);
                }
                if let Some(joined) = pool.tasks.join_next().await {
                    joined.expect("evicted task exits");
                }
            }
        }
        pool.shutdown().await;
    }

    #[tokio::test]
    async fn stale_close_generation_cannot_remove_or_fail_its_live_replacement() {
        let (first_socket, mut first_peer) = tokio::io::duplex(1024);
        let (replacement_socket, _replacement_peer) = tokio::io::duplex(1024);
        let (tx, mut events) = mpsc::channel(8);
        let mut pool = Pool::new(PoolConfig::default(), Limits::stream(), tx);
        let key = tcp("127.0.0.1:5093".parse().expect("address"));

        pool.insert_stream(first_socket, key.clone(), Origin::Inbound)
            .expect("first generation starts");
        let old_id = pool.connections.get(&key).expect("registered").id;
        pool.retire(&key);
        let mut byte = [0u8; 1];
        assert_eq!(first_peer.read(&mut byte).await.expect("EOF"), 0);
        let Event::Closed { id, .. } = events.recv().await.expect("old close arrives") else {
            panic!("expected old close");
        };
        assert_eq!(id, old_id);

        pool.insert_stream(replacement_socket, key.clone(), Origin::Inbound)
            .expect("replacement starts after old task exits");
        let replacement_id = pool.connections.get(&key).expect("replacement held").id;
        assert_ne!(replacement_id, old_id);

        // Retired generations remain entitled to exactly one close acknowledgement. Side effects
        // are generation-scoped by the driver, so acknowledging this one cannot touch replacement.
        assert!(pool.remove(&key, old_id), "the retirement is acknowledged");
        assert!(
            !pool.remove(&key, old_id),
            "the acknowledgement is exactly once"
        );
        assert!(pool.holds(&key), "the replacement remains routable");
        pool.shutdown().await;
    }

    #[tokio::test]
    async fn same_key_replacement_at_capacity_does_not_evict_an_unrelated_connection() {
        let (a_socket, mut a_peer) = tokio::io::duplex(1024);
        let (b_socket, mut b_peer) = tokio::io::duplex(1024);
        let (replacement_socket, _replacement_peer) = tokio::io::duplex(1024);
        let (tx, mut events) = mpsc::channel(8);
        let mut pool = Pool::new(
            PoolConfig {
                max_connections: 2,
                ..PoolConfig::default()
            },
            Limits::stream(),
            tx,
        );
        let a = tcp("127.0.0.1:5094".parse().expect("address"));
        let b = tcp("127.0.0.1:5095".parse().expect("address"));
        pool.insert_stream(a_socket, a.clone(), Origin::Inbound)
            .expect("A starts");
        pool.insert_stream(b_socket, b.clone(), Origin::Inbound)
            .expect("B starts");

        pool.insert_stream(replacement_socket, a, Origin::Inbound)
            .expect("A replacement is reserved while the old task retires");

        let mut byte = [0u8; 1];
        assert_eq!(a_peer.read(&mut byte).await.expect("A closes"), 0);
        let Event::Closed { key, id } = events.recv().await.expect("A close arrives") else {
            panic!("expected close");
        };
        assert!(pool.remove(&key, id));
        assert!(pool.holds(&b), "B must not be selected as a second victim");
        assert!(
            pool.send_on_existing(&b, Bytes::from_static(b"b")).await,
            "B remains writable"
        );
        assert_eq!(b_peer.read(&mut byte).await.expect("B receives"), 1);
        assert_eq!(byte[0], b'b');
        pool.shutdown().await;
    }

    #[tokio::test]
    async fn public_send_reserves_same_key_replacement_without_evicting_b() {
        let (a_socket, mut a_peer) = tokio::io::duplex(1024);
        let (b_socket, mut b_peer) = tokio::io::duplex(1024);
        let (events, mut closed) = mpsc::channel(8);
        let mut pool = Pool::new(
            PoolConfig {
                max_connections: 2,
                ..PoolConfig::default()
            },
            Limits::stream(),
            events,
        );
        let a = tcp("127.0.0.1:59991".parse().expect("address"));
        let b = tcp("127.0.0.1:59992".parse().expect("address"));
        pool.insert_stream(a_socket, a.clone(), Origin::Inbound)
            .expect("A starts inbound");
        pool.insert_stream(b_socket, b.clone(), Origin::Inbound)
            .expect("B starts inbound");
        let old_a = pool.generation(&a).expect("A generation");

        pool.send(&a, Bytes::from_static(b"replacement"))
            .await
            .expect("public send reserves outbound A");
        let replacement = pool.generation(&a).expect("replacement generation");
        assert_ne!(replacement, old_a);
        assert!(pool.holds(&b), "B is not selected as a second victim");
        let mut byte = [0u8; 1];
        assert_eq!(a_peer.read(&mut byte).await.expect("old A closes"), 0);
        let Event::Closed { key, id } = closed.recv().await.expect("old A reports close") else {
            panic!("expected close");
        };
        assert_eq!((&key, id), (&a, old_a));
        assert!(pool.remove(&key, id));
        assert!(pool.send_on_existing(&b, Bytes::from_static(b"b")).await);
        assert_eq!(b_peer.read(&mut byte).await.expect("B reads"), 1);
        pool.shutdown().await;
    }

    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn public_send_tls_reserves_same_key_replacement_without_evicting_b() {
        use sipx_testkit::certs::Ca;

        use crate::tls::{ClientTls, TrustAnchors};

        let ca = Ca::new();
        let mut anchors = TrustAnchors::only();
        anchors.add_pem(ca.pem().as_bytes()).expect("CA loads");
        let client = ClientTls::new(&anchors).expect("TLS client");
        let (a_socket, mut a_peer) = tokio::io::duplex(1024);
        let (b_socket, _b_peer) = tokio::io::duplex(1024);
        let (events, _closed) = mpsc::channel(8);
        let mut pool = Pool::new(
            PoolConfig {
                max_connections: 2,
                ..PoolConfig::default()
            },
            Limits::stream(),
            events,
        );
        let a = ConnectionKey::new(
            "127.0.0.1:59993".parse().expect("address"),
            TransportKind::Tls,
        );
        let b = tcp("127.0.0.1:59994".parse().expect("address"));
        pool.insert_stream(a_socket, a.clone(), Origin::Inbound)
            .expect("A starts inbound");
        pool.insert_stream(b_socket, b.clone(), Origin::Inbound)
            .expect("B starts inbound");

        pool.send_tls(&a, "localhost", &client, Bytes::from_static(b"replacement"))
            .await
            .expect("public TLS send reserves outbound A");
        assert!(pool.holds(&b));
        let mut byte = [0u8; 1];
        assert_eq!(a_peer.read(&mut byte).await.expect("old A closes"), 0);
        pool.shutdown().await;
    }

    #[cfg(feature = "ws")]
    #[tokio::test]
    async fn public_send_ws_reserves_same_key_replacement_without_evicting_b() {
        let (a_socket, mut a_peer) = tokio::io::duplex(1024);
        let (b_socket, _b_peer) = tokio::io::duplex(1024);
        let (events, _closed) = mpsc::channel(8);
        let mut pool = Pool::new(
            PoolConfig {
                max_connections: 2,
                ..PoolConfig::default()
            },
            Limits::stream(),
            events,
        );
        let a = ConnectionKey::new(
            "127.0.0.1:59995".parse().expect("address"),
            TransportKind::Ws,
        );
        let b = tcp("127.0.0.1:59996".parse().expect("address"));
        pool.insert_stream(a_socket, a.clone(), Origin::Inbound)
            .expect("A starts inbound");
        pool.insert_stream(b_socket, b.clone(), Origin::Inbound)
            .expect("B starts inbound");

        pool.send_ws(
            &a,
            "localhost",
            Duration::from_secs(60),
            #[cfg(feature = "wss")]
            None,
            Bytes::from_static(b"replacement"),
        )
        .await
        .expect("public WebSocket send reserves outbound A");
        assert!(pool.holds(&b));
        let mut byte = [0u8; 1];
        assert_eq!(a_peer.read(&mut byte).await.expect("old A closes"), 0);
        pool.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_cancels_a_final_close_blocked_on_a_full_event_channel() {
        let (events, _unread) = mpsc::channel(1);
        events
            .send(Event::FramingFailed {
                key: tcp("127.0.0.1:59997".parse().expect("address")),
            })
            .await
            .expect("fills event channel");
        let (socket, mut peer) = tokio::io::duplex(1024);
        let mut pool = Pool::new(PoolConfig::default(), Limits::stream(), events);
        let key = tcp("127.0.0.1:59998".parse().expect("address"));
        pool.insert_stream(socket, key.clone(), Origin::Inbound)
            .expect("connection starts");
        pool.retire(&key);
        let mut byte = [0u8; 1];
        assert_eq!(peer.read(&mut byte).await.expect("retirement closes"), 0);
        tokio::task::yield_now().await;

        tokio::time::timeout(Duration::from_secs(1), pool.shutdown())
            .await
            .expect("shutdown cancels the blocked final event sender");
    }
}
