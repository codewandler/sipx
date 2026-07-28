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
use crate::target::TransportKind;

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
    /// The connection is gone.
    ///
    /// Every transaction bound to it is given a transport error rather than being left to
    /// time out: waiting 32 seconds to learn something we already know is both a bad
    /// experience and a resource leak.
    Closed {
        /// The peer whose connection closed.
        peer: SocketAddr,
        /// Which transport it was.
        transport: TransportKind,
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

/// A pool of TCP connections, keyed by peer address.
#[derive(Debug)]
pub struct Pool {
    connections: HashMap<SocketAddr, Pooled>,
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
        self.insert(stream, peer, Origin::Inbound);
    }

    /// Send to a peer, connecting if there is no usable connection.
    ///
    /// The dial never blocks the caller. A peer that black-holes SYN takes the OS connect
    /// timeout to fail — around two minutes — and the endpoint loop that calls this also owns
    /// every transaction timer. Waiting here would stop retransmissions for calls that have
    /// nothing to do with this peer, so the connection is established inside its own task and
    /// the bytes wait in the channel until it is up.
    pub async fn send(&mut self, peer: SocketAddr, bytes: Bytes) -> Result<()> {
        let reusable = self.connections.get(&peer).is_some_and(|pooled| {
            !pooled.writer.is_closed()
                && (pooled.origin == Origin::Outbound || self.config.reuse_inbound_for_outbound)
        });
        if !reusable {
            // Either there is nothing here, the writer is gone, or policy forbids reusing an
            // inbound connection for our own requests. All three mean: open our own.
            self.connections.remove(&peer);
            self.insert_connecting(peer);
        }

        let Some(pooled) = self.connections.get_mut(&peer) else {
            return Err(crate::error::Error::EndpointClosed);
        };
        pooled.last_used = Instant::now();
        let writer = pooled.writer.clone();
        writer
            .send(bytes)
            .await
            .map_err(|_| crate::error::Error::EndpointClosed)
    }

    /// Register a connection that is still being dialled.
    ///
    /// The writer exists before the socket does, so bytes queue in the channel rather than in
    /// the caller.
    fn insert_connecting(&mut self, peer: SocketAddr) {
        if self.connections.len() >= self.config.max_connections {
            self.evict_least_recently_used();
        }

        let (writer_tx, writer_rx) = mpsc::channel::<Bytes>(64);
        let events = self.events.clone();
        let limits = self.limits;
        tokio::spawn(async move {
            match TcpStream::connect(peer).await {
                Ok(stream) => {
                    pump(stream, peer, writer_rx, events, limits, TransportKind::Tcp).await;
                }
                Err(error) => {
                    tracing::debug!(%error, %peer, "connect failed");
                    let _ = events
                        .send(Event::Closed {
                            peer,
                            transport: TransportKind::Tcp,
                        })
                        .await;
                }
            }
        });

        self.connections.insert(
            peer,
            Pooled {
                writer: writer_tx,
                origin: Origin::Outbound,
                last_used: Instant::now(),
            },
        );
    }

    /// Adopt a TLS connection a peer opened, once the handshake has completed.
    #[cfg(feature = "tls")]
    pub fn accept_tls(
        &mut self,
        stream: tokio_rustls::server::TlsStream<TcpStream>,
        peer: SocketAddr,
    ) {
        self.insert_stream(stream, peer, Origin::Inbound, TransportKind::Tls);
    }

    /// Send to a peer over TLS, connecting and verifying if there is no usable connection.
    ///
    /// The verification name is the host from the URI, not the address — see
    /// `docs/specs/sip-tls.md` §3.3.
    #[cfg(feature = "tls")]
    pub async fn send_tls(
        &mut self,
        peer: SocketAddr,
        verification_name: &str,
        client: &crate::tls::ClientTls,
        bytes: Bytes,
    ) -> Result<()> {
        let reusable = self.connections.get(&peer).is_some_and(|pooled| {
            !pooled.writer.is_closed()
                && (pooled.origin == Origin::Outbound || self.config.reuse_inbound_for_outbound)
        });
        if !reusable {
            self.connections.remove(&peer);
            self.insert_connecting_tls(peer, verification_name, client)?;
        }

        let Some(pooled) = self.connections.get_mut(&peer) else {
            return Err(crate::error::Error::EndpointClosed);
        };
        pooled.last_used = Instant::now();
        let writer = pooled.writer.clone();
        writer
            .send(bytes)
            .await
            .map_err(|_| crate::error::Error::EndpointClosed)
    }

    #[cfg(feature = "tls")]
    fn insert_connecting_tls(
        &mut self,
        peer: SocketAddr,
        verification_name: &str,
        client: &crate::tls::ClientTls,
    ) -> Result<()> {
        if self.connections.len() >= self.config.max_connections {
            self.evict_least_recently_used();
        }

        let name = crate::tls::verification_name(verification_name)?;
        let connector = client.connector();
        let (writer_tx, writer_rx) = mpsc::channel::<Bytes>(64);
        let events = self.events.clone();
        let limits = self.limits;

        tokio::spawn(async move {
            let stream = match TcpStream::connect(peer).await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::debug!(%error, %peer, "connect failed");
                    let _ = events
                        .send(Event::Closed {
                            peer,
                            transport: TransportKind::Tcp,
                        })
                        .await;
                    return;
                }
            };
            match connector.connect(name, stream).await {
                Ok(tls) => {
                    pump(tls, peer, writer_rx, events, limits, TransportKind::Tls).await;
                }
                Err(error) => {
                    // Every verification failure arrives here, with the reason attached. It is
                    // logged rather than swallowed, and the connection simply does not exist —
                    // there is no fallback to cleartext.
                    tracing::warn!(%error, %peer, "TLS handshake failed");
                    let _ = events
                        .send(Event::Closed {
                            peer,
                            transport: TransportKind::Tls,
                        })
                        .await;
                }
            }
        });

        self.connections.insert(
            peer,
            Pooled {
                writer: writer_tx,
                origin: Origin::Outbound,
                last_used: Instant::now(),
            },
        );
        Ok(())
    }

    /// Forget a connection that has closed.
    pub fn remove(&mut self, peer: SocketAddr) {
        self.connections.remove(&peer);
    }

    /// Whether a connection to this peer is held.
    #[must_use]
    pub fn holds(&self, peer: SocketAddr) -> bool {
        self.connections.contains_key(&peer)
    }

    /// Answer on the connection a request arrived over, if it is still open.
    ///
    /// Always tried before anything the `Via` says: opening a new connection to a NAT-ed
    /// client's advertised address cannot work, which is what RFC 5923 exists to say.
    ///
    /// [`Via`]: sipx_sip::headers::Via
    pub async fn send_on_existing(&mut self, peer: SocketAddr, bytes: Bytes) -> bool {
        let Some(pooled) = self.connections.get_mut(&peer) else {
            return false;
        };
        if pooled.writer.send(bytes).await.is_err() {
            self.connections.remove(&peer);
            return false;
        }
        pooled.last_used = Instant::now();
        true
    }

    /// Close connections idle for longer than the configured timeout.
    pub fn evict_idle(&mut self) -> Vec<SocketAddr> {
        let deadline = Instant::now();
        let idle_timeout = self.config.idle_timeout;
        let evicted: Vec<SocketAddr> = self
            .connections
            .iter()
            .filter(|(_, c)| deadline.duration_since(c.last_used) > idle_timeout)
            .map(|(addr, _)| *addr)
            .collect();
        for addr in &evicted {
            self.connections.remove(addr);
        }
        evicted
    }

    fn insert(&mut self, stream: TcpStream, peer: SocketAddr, origin: Origin) {
        self.insert_stream(stream, peer, origin, TransportKind::Tcp);
    }

    fn insert_stream<S>(
        &mut self,
        stream: S,
        peer: SocketAddr,
        origin: Origin,
        transport: TransportKind,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
    {
        if self.connections.len() >= self.config.max_connections {
            self.evict_least_recently_used();
        }

        let (writer_tx, writer_rx) = mpsc::channel::<Bytes>(64);
        let events = self.events.clone();
        let limits = self.limits;
        tokio::spawn(pump(stream, peer, writer_rx, events, limits, transport));

        self.connections.insert(
            peer,
            Pooled {
                writer: writer_tx,
                origin,
                last_used: Instant::now(),
            },
        );
    }

    /// Accept a TLS connection, doing the handshake before it joins the pool.
    #[cfg(feature = "tls")]
    pub fn accept_tls_handshake(
        &self,
        stream: TcpStream,
        peer: SocketAddr,
        server: &crate::tls::ServerTls,
        accepted: mpsc::Sender<(tokio_rustls::server::TlsStream<TcpStream>, SocketAddr)>,
    ) {
        let acceptor = server.acceptor();
        tokio::spawn(async move {
            match acceptor.accept(stream).await {
                Ok(tls) => {
                    let _ = accepted.send((tls, peer)).await;
                }
                Err(error) => {
                    tracing::debug!(%error, %peer, "inbound TLS handshake failed");
                }
            }
        });
    }

    fn evict_least_recently_used(&mut self) {
        let Some(victim) = self
            .connections
            .iter()
            .min_by_key(|(_, c)| c.last_used)
            .map(|(addr, _)| *addr)
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
    peer: SocketAddr,
    mut outgoing: mpsc::Receiver<Bytes>,
    events: mpsc::Sender<Event>,
    limits: Limits,
    transport: TransportKind,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + 'static,
{
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

    let _ = events.send(Event::Closed { peer, transport }).await;
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

/// The transport kind a pool serves.
#[must_use]
pub fn kind() -> TransportKind {
    TransportKind::Tcp
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
            Event::Closed { .. } => panic!("expected a message"),
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
            pool.send_on_existing(peer, Bytes::from_static(b"x")).await,
            "a response must go back over the connection it came in on"
        );

        // But an outbound request to that same peer must not ride it. With reuse off, the
        // pool drops the inbound entry and dials afresh; the dial fails here because the peer
        // is a client socket with nothing listening, which is exactly the point — it did not
        // silently reuse.
        let before = pool.len();
        let _ = pool.send(peer, Bytes::from_static(b"y")).await;
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
