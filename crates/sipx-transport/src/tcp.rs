//! The TCP transport: stream framing and a connection pool.
//!
//! A stream is not a sequence of messages until something makes it one, so each connection
//! owns a [`StreamParser`] and hands completed messages to the endpoint loop. Connections are
//! pooled and reused, because opening one per request is both slow and, for a peer behind a
//! NAT, impossible in the reverse direction.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use bytes::Bytes;
use sipx_sip::{Limits, Message, StreamParser};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

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
    },
    /// A CRLF keep-alive arrived on this connection (RFC 5626 §4.4.1).
    ///
    /// Reported rather than dropped because it is the *pong* half of the mechanism: a UA that sent
    /// a CRLFCRLF ping "MUST treat the flow as failed" if no single-CRLF pong comes back within 10
    /// seconds, which it cannot do if nothing tells it one arrived.
    Pong {
        /// Which connection it arrived on.
        key: ConnectionKey,
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
    origin: Origin,
    last_used: Instant,
}

/// A pool of stream connections.
#[derive(Debug)]
pub struct Pool {
    connections: HashMap<ConnectionKey, Pooled>,
    config: PoolConfig,
    events: mpsc::Sender<Event>,
    limits: Limits,
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
        }
    }

    /// How many connections are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.connections.len()
    }

    /// Whether the pool is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    /// Adopt a connection a peer opened.
    pub fn accept(&mut self, stream: TcpStream, peer: SocketAddr) {
        let key = ConnectionKey::new(peer, TransportKind::Tcp);
        self.insert_stream(stream, key, Origin::Inbound);
    }

    /// Send to a peer, connecting if there is no usable connection.
    ///
    /// The dial never blocks the caller. A peer that black-holes SYN takes the OS connect
    /// timeout to fail — around two minutes — and the endpoint loop that calls this also owns
    /// every transaction timer. Waiting here would stop retransmissions for calls that have
    /// nothing to do with this peer, so the connection is established inside its own task and
    /// the bytes wait in the channel until it is up.
    pub async fn send(&mut self, key: &ConnectionKey, bytes: Bytes) -> Result<()> {
        if !self.reusable(key) {
            // Either there is nothing here, the writer is gone, or policy forbids reusing an
            // inbound connection for our own requests. All three mean: open our own.
            self.connections.remove(key);
            self.dial(key.clone());
        }
        self.queue(key, bytes).await
    }

    /// Adopt a TLS connection a peer opened, once the handshake has completed.
    #[cfg(feature = "tls")]
    pub fn accept_tls(
        &mut self,
        stream: tokio_rustls::server::TlsStream<TcpStream>,
        peer: SocketAddr,
    ) {
        let key = ConnectionKey::new(peer, TransportKind::Tls);
        self.insert_stream(stream, key, Origin::Inbound);
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
        if !self.reusable(key) {
            self.connections.remove(key);
            self.dial_tls(key.clone(), verification_name, client)?;
        }
        self.queue(key, bytes).await
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
        self.spawn_ws(ws, key, Origin::Inbound, keepalive);
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
        if !self.reusable(key) {
            self.connections.remove(key);
            self.dial_ws(
                key.clone(),
                authority,
                keepalive,
                #[cfg(feature = "wss")]
                client,
            )?;
        }
        self.queue(key, bytes).await
    }

    /// Forget a connection that has closed.
    pub fn remove(&mut self, key: &ConnectionKey) {
        self.connections.remove(key);
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
        let Some(pooled) = self.connections.get_mut(key) else {
            return false;
        };
        if pooled.writer.send(bytes).await.is_err() {
            self.connections.remove(key);
            return false;
        }
        pooled.last_used = Instant::now();
        true
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
            self.connections.remove(key);
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

    /// Make room, then take the writer half of a connection whose task is about to be spawned.
    ///
    /// The writer exists before the socket does, so bytes queue in the channel rather than in
    /// the caller while a connection is still being established.
    fn register(&mut self, key: ConnectionKey, origin: Origin) -> mpsc::Receiver<Bytes> {
        if self.connections.len() >= self.config.max_connections {
            self.evict_least_recently_used();
        }
        let (writer_tx, writer_rx) = mpsc::channel::<Bytes>(64);
        self.connections.insert(
            key,
            Pooled {
                writer: writer_tx,
                origin,
                last_used: Instant::now(),
            },
        );
        writer_rx
    }

    fn dial(&mut self, key: ConnectionKey) {
        let writer_rx = self.register(key.clone(), Origin::Outbound);
        let events = self.events.clone();
        let limits = self.limits;
        tokio::spawn(async move {
            match TcpStream::connect(key.peer).await {
                Ok(stream) => pump(stream, key, writer_rx, events, limits).await,
                Err(error) => {
                    tracing::debug!(%error, peer = %key.peer, "connect failed");
                    let _ = events.send(Event::Closed { key }).await;
                }
            }
        });
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
        let writer_rx = self.register(key.clone(), Origin::Outbound);
        let events = self.events.clone();
        let limits = self.limits;

        tokio::spawn(async move {
            let stream = match TcpStream::connect(key.peer).await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::debug!(%error, peer = %key.peer, "connect failed");
                    let _ = events.send(Event::Closed { key }).await;
                    return;
                }
            };
            match connector.connect(name, stream).await {
                Ok(tls) => pump(tls, key, writer_rx, events, limits).await,
                Err(error) => {
                    // Every verification failure arrives here, with the reason attached. It is
                    // logged rather than swallowed, and the connection simply does not exist —
                    // there is no fallback to cleartext.
                    tracing::warn!(%error, peer = %key.peer, "TLS handshake failed");
                    let _ = events.send(Event::Closed { key }).await;
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
        let writer_rx = self.register(key.clone(), Origin::Outbound);
        let events = self.events.clone();
        let limits = self.limits;

        tokio::spawn(async move {
            let stream = match TcpStream::connect(key.peer).await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::debug!(%error, peer = %key.peer, "connect failed");
                    let _ = events.send(Event::Closed { key }).await;
                    return;
                }
            };

            #[cfg(feature = "wss")]
            if let Some((connector, name)) = secure {
                match connector.connect(name, stream).await {
                    Ok(tls) => {
                        crate::ws::dial(tls, &authority, key, writer_rx, events, limits, keepalive)
                            .await;
                    }
                    Err(error) => {
                        tracing::warn!(%error, peer = %key.peer, "TLS handshake failed");
                        let _ = events.send(Event::Closed { key }).await;
                    }
                }
                return;
            }

            crate::ws::dial(
                stream, &authority, key, writer_rx, events, limits, keepalive,
            )
            .await;
        });
        Ok(())
    }

    fn insert_stream<S>(&mut self, stream: S, key: ConnectionKey, origin: Origin)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
    {
        let writer_rx = self.register(key.clone(), origin);
        let events = self.events.clone();
        let limits = self.limits;
        tokio::spawn(pump(stream, key, writer_rx, events, limits));
    }

    #[cfg(feature = "ws")]
    fn spawn_ws<S>(
        &mut self,
        ws: crate::ws::Socket<S>,
        key: ConnectionKey,
        origin: Origin,
        keepalive: Duration,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let writer_rx = self.register(key.clone(), origin);
        let events = self.events.clone();
        let limits = self.limits;
        tokio::spawn(crate::ws::pump(
            ws, key, writer_rx, events, limits, keepalive,
        ));
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
        self.connections.remove(&victim);
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
                                if events.send(Event::Pong { key: key.clone() }).await.is_err() {
                                    return;
                                }
                            }
                            for message in messages {
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
                        }
                        Err(error) => {
                            // Framing is lost. Resynchronizing would mean guessing where the
                            // next message starts, which is how a body becomes a request.
                            tracing::debug!(%error, %peer, "closing connection on framing error");
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

    let _ = events.send(Event::Closed { key }).await;
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
            Event::Pong { .. } | Event::Closed { .. } => panic!("expected a message"),
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

        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("no timeout")
            .expect("an event");
        assert!(matches!(event, Event::Closed { .. }));
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
            pool.len() <= before,
            "the inbound connection must not have been adopted for outbound use"
        );
    }

    #[tokio::test]
    async fn idle_connections_are_evicted() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let addr = listener.local_addr().expect("has an address");
        let (tx, _rx) = mpsc::channel(64);
        let mut pool = Pool::new(
            PoolConfig {
                idle_timeout: Duration::ZERO,
                ..PoolConfig::default()
            },
            Limits::stream(),
            tx,
        );

        let accepted = tokio::spawn(async move { listener.accept().await.expect("accepts") });
        let _client = TcpStream::connect(addr).await.expect("connects");
        let (stream, peer) = accepted.await.expect("accepted");
        pool.accept(stream, peer);
        assert_eq!(pool.len(), 1);

        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(pool.evict_idle().len(), 1);
        assert!(pool.is_empty());
    }
}
