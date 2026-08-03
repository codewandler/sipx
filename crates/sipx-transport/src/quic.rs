//! Experimental SIP-over-QUIC mapping from `docs/specs/sip-quic.md`.
//!
//! QUIC is connection-oriented for transaction timers but message-oriented for framing: each
//! request gets one bidirectional stream, and responses return on that same stream. Certificate
//! policy is built in [`crate::tls`] and converted here; this module never installs a verifier.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use sipx_sip::{Limits, Message, parse_datagram};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::error::Result;
use crate::target::{ConnectionKey, TransportKind};
use crate::tcp::Event;

/// What failed while establishing or using a QUIC connection.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum QuicError {
    /// The certificate did not cover the URI host.
    #[error("certificate for {peer} has the wrong host: {detail}")]
    WrongHost {
        /// Peer being authenticated.
        peer: String,
        /// Verification detail without certificate material.
        detail: String,
    },
    /// No configured trust anchor vouched for the certificate.
    #[error("certificate for {peer} has an unknown issuer: {detail}")]
    UnknownIssuer {
        /// Peer being authenticated.
        peer: String,
        /// Verification detail without certificate material.
        detail: String,
    },
    /// The peer did not negotiate the required `sip/2` application protocol.
    #[error("QUIC peer {peer} negotiated the wrong ALPN: {detail}")]
    WrongAlpn {
        /// Peer being authenticated.
        peer: String,
        /// Negotiation detail.
        detail: String,
    },
    /// The authenticated connection closed.
    #[error("QUIC connection to {peer} closed: {detail}")]
    ConnectionClosed {
        /// Peer whose connection closed.
        peer: String,
        /// Close code and reason.
        detail: String,
    },
    /// Another handshake failure.
    #[error("QUIC handshake with {peer}: {detail}")]
    Handshake {
        /// Peer being authenticated.
        peer: String,
        /// Backend detail.
        detail: String,
    },
}

impl QuicError {
    pub(crate) fn handshake(peer: String, detail: String) -> Self {
        let folded = detail.to_ascii_lowercase();
        if folded.contains("notvalidforname") || folded.contains("not valid for") {
            Self::WrongHost { peer, detail }
        } else if folded.contains("unknownissuer") || folded.contains("unknown issuer") {
            Self::UnknownIssuer { peer, detail }
        } else if folded.contains("application protocol")
            || folded.contains("known protocol")
            || folded.contains("alpn")
        {
            Self::WrongAlpn { peer, detail }
        } else {
            Self::Handshake { peer, detail }
        }
    }
}

/// A route back to the send half of the exact stream carrying an inbound request.
pub(crate) type Reply = mpsc::Sender<Bytes>;

const KEEPALIVE: Duration = Duration::from_secs(25);

pub(crate) fn transport_config() -> Arc<quinn::TransportConfig> {
    let mut transport = quinn::TransportConfig::default();
    transport.keep_alive_interval(Some(KEEPALIVE));
    transport.max_concurrent_bidi_streams(64_u8.into());
    transport.max_concurrent_uni_streams(0_u8.into());
    Arc::new(transport)
}

/// Bind the UDP socket Quinn owns, optionally accepting incoming connections.
pub(crate) fn endpoint(
    ip: IpAddr,
    port: u16,
    client: Option<&crate::tls::ClientTls>,
    server: Option<&crate::tls::ServerTls>,
) -> Result<Endpoint> {
    let bind = SocketAddr::new(ip, port);
    let mut endpoint = match server {
        Some(server) => {
            let mut config = server.quic_config()?;
            config.transport_config(transport_config());
            Endpoint::server(config, bind)?
        }
        None => Endpoint::client(bind)?,
    };
    if let Some(client) = client {
        let mut config = client.quic_config()?;
        config.transport_config(transport_config());
        endpoint.set_default_client_config(config);
    }
    Ok(endpoint)
}

/// Drive all streams on one authenticated QUIC connection.
pub(crate) async fn pump(
    connection: Connection,
    key: ConnectionKey,
    id: u64,
    mut outgoing: mpsc::Receiver<Bytes>,
    events: mpsc::Sender<Event>,
    limits: Limits,
) {
    let mut streams = JoinSet::new();
    let detail = loop {
        tokio::select! {
            incoming = connection.accept_bi() => match incoming {
                Ok((send, recv)) => {
                    let events = events.clone();
                    let key = key.clone();
                    let connection = connection.clone();
                    streams.spawn(async move {
                        receive_request(connection, send, recv, key, id, events, limits).await;
                    });
                }
                Err(error) => break error.to_string(),
            },
            Some(bytes) = outgoing.recv() => {
                let connection = connection.clone();
                let events = events.clone();
                let key = key.clone();
                streams.spawn(async move {
                    send_request(connection, bytes, key, id, events, limits).await;
                });
            }
            completed = streams.join_next(), if !streams.is_empty() => {
                if let Some(Err(error)) = completed {
                    tracing::debug!(%error, peer = %key.peer, "QUIC stream task failed");
                }
            }
            error = connection.closed() => break error.to_string(),
            else => break "connection task ended".to_owned(),
        }
    };
    streams.abort_all();
    while streams.join_next().await.is_some() {}
    // discard: a closed event channel means the endpoint already stopped and owns no transactions.
    let _ = events.send(Event::QuicClosed { key, id, detail }).await;
}

async fn receive_request(
    connection: Connection,
    send: SendStream,
    mut recv: RecvStream,
    key: ConnectionKey,
    id: u64,
    events: mpsc::Sender<Event>,
    limits: Limits,
) {
    let peer = key.peer;
    let bytes = match recv.read_to_end(limits.max_message_bytes).await {
        Ok(bytes) => Bytes::from(bytes),
        Err(quinn::ReadToEndError::Read(quinn::ReadError::ConnectionLost(error))) => {
            tracing::debug!(%error, %peer, "QUIC connection closed while reading a request");
            return;
        }
        Err(error) => {
            tracing::debug!(%error, %peer, "QUIC request stream could not be read");
            // discard: if the driver stopped, no counter or transaction remains to receive this.
            let _ = events.send(Event::FramingFailed { key }).await;
            connection.close(1_u8.into(), b"malformed SIP stream");
            return;
        }
    };
    let message = match parse_one(bytes, &limits) {
        Ok(message @ Message::Request(_)) => message,
        Ok(Message::Response(_)) => {
            tracing::debug!(%peer, "QUIC peer opened a stream with a response");
            // discard: if the driver stopped, no counter or transaction remains to receive this.
            let _ = events.send(Event::FramingFailed { key }).await;
            connection.close(1_u8.into(), b"response opened a request stream");
            return;
        }
        Err(error) => {
            // discard: the malformed stream itself; closing the connection is the mandated result.
            tracing::debug!(%error, %peer, "malformed QUIC request stream");
            // discard: if the driver stopped, no counter or transaction remains to receive this.
            let _ = events.send(Event::FramingFailed { key }).await;
            connection.close(1_u8.into(), b"malformed SIP stream");
            return;
        }
    };
    let (reply, replies) = mpsc::channel(16);
    if events
        .send(Event::Message {
            message: Box::new(message),
            source: peer,
            transport: TransportKind::Quic,
            id,
            quic_reply: Some(reply),
        })
        .await
        .is_ok()
    {
        write_responses(send, replies).await;
    }
}

async fn write_responses(mut send: SendStream, mut replies: mpsc::Receiver<Bytes>) {
    while let Some(bytes) = replies.recv().await {
        let final_response = parse_datagram(bytes.clone(), &Limits::stream())
            .ok()
            .and_then(|message| match message {
                Message::Response(response) => Some(response.status.is_final()),
                Message::Request(_) => None,
            })
            .unwrap_or(true);
        if send.write_all(&bytes).await.is_err() {
            return;
        }
        if final_response {
            // discard: the response bytes were written; a finish error means the peer closed first.
            let _ = send.finish();
            return;
        }
    }
    // discard: no response sender remains; finishing is best-effort because the peer may be gone.
    let _ = send.finish();
}

async fn send_request(
    connection: Connection,
    bytes: Bytes,
    key: ConnectionKey,
    id: u64,
    events: mpsc::Sender<Event>,
    limits: Limits,
) {
    let peer = key.peer;
    let Some(mut recv) = write_request(&connection, &events, &key, bytes).await else {
        return;
    };

    let mut pending = BytesMut::new();
    let mut final_response = None;
    loop {
        let chunk = match recv.read_chunk(usize::MAX, true).await {
            Ok(Some(chunk)) => chunk.bytes,
            Ok(None) => break,
            Err(quinn::ReadError::ConnectionLost(error)) => {
                tracing::debug!(%error, %peer, "QUIC connection closed while reading a response");
                return;
            }
            Err(error) => {
                reject_response_stream(&connection, &events, &key, peer, error.to_string()).await;
                return;
            }
        };
        if final_response.is_some() {
            reject_response_stream(
                &connection,
                &events,
                &key,
                peer,
                "bytes followed the final response".to_owned(),
            )
            .await;
            return;
        }
        pending.extend_from_slice(&chunk);
        loop {
            let message = match take_response(&mut pending, false, &limits) {
                Ok(Some(message)) => message,
                Ok(None) => break,
                Err(error) => {
                    reject_response_stream(&connection, &events, &key, peer, error).await;
                    return;
                }
            };
            let is_final = match &message {
                Message::Response(response) => response.status.is_final(),
                Message::Request(_) => {
                    reject_response_stream(
                        &connection,
                        &events,
                        &key,
                        peer,
                        "request on response stream".to_owned(),
                    )
                    .await;
                    return;
                }
            };
            if is_final {
                final_response = Some(message);
                if !pending.is_empty() {
                    reject_response_stream(
                        &connection,
                        &events,
                        &key,
                        peer,
                        "bytes followed the final response".to_owned(),
                    )
                    .await;
                    return;
                }
                break;
            }
            if send_response_event(&events, message, peer, id)
                .await
                .is_err()
            {
                return;
            }
        }
    }

    let message = match complete_response(final_response, &mut pending, &limits) {
        Ok(message) => message,
        Err(error) => {
            reject_response_stream(&connection, &events, &key, peer, error).await;
            return;
        }
    };
    // discard: if the driver stopped, no transaction remains to receive this response.
    let _ = send_response_event(&events, message, peer, id).await;
}

async fn write_request(
    connection: &Connection,
    events: &mpsc::Sender<Event>,
    key: &ConnectionKey,
    bytes: Bytes,
) -> Option<RecvStream> {
    let peer = key.peer;
    let (mut send, recv) = match connection.open_bi().await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::debug!(%error, %peer, "QUIC stream could not be opened");
            return None;
        }
    };
    if let Err(error) = send.write_all(&bytes).await {
        match error {
            quinn::WriteError::ConnectionLost(error) => {
                tracing::debug!(%error, %peer, "QUIC connection closed while writing a request");
            }
            other => {
                reject_response_stream(connection, events, key, peer, other.to_string()).await;
            }
        }
        return None;
    }
    if let Err(error) = send.finish() {
        tracing::debug!(%error, %peer, "QUIC request stream closed before it could finish");
        return None;
    }
    Some(recv)
}

fn complete_response(
    final_response: Option<Message>,
    pending: &mut BytesMut,
    limits: &Limits,
) -> std::result::Result<Message, String> {
    if let Some(message) = final_response {
        return pending
            .is_empty()
            .then_some(message)
            .ok_or_else(|| "bytes followed the final response".to_owned());
    }
    match take_response(pending, true, limits)? {
        Some(message)
            if matches!(&message, Message::Response(response) if response.status.is_final())
                && pending.is_empty() =>
        {
            Ok(message)
        }
        Some(_) | None => Err("response stream ended without one final response".to_owned()),
    }
}

async fn send_response_event(
    events: &mpsc::Sender<Event>,
    message: Message,
    peer: SocketAddr,
    id: u64,
) -> std::result::Result<(), mpsc::error::SendError<Event>> {
    events
        .send(Event::Message {
            message: Box::new(message),
            source: peer,
            transport: TransportKind::Quic,
            id,
            quic_reply: None,
        })
        .await
}

async fn reject_response_stream(
    connection: &Connection,
    events: &mpsc::Sender<Event>,
    key: &ConnectionKey,
    peer: SocketAddr,
    detail: String,
) {
    // discard: a malformed peer is closed deliberately, and if the driver has stopped no counter
    // or transaction remains to receive the framing-failure event.
    tracing::debug!(%detail, %peer, "malformed QUIC response stream");
    let _ = events.send(Event::FramingFailed { key: key.clone() }).await;
    connection.close(1_u8.into(), b"malformed SIP stream");
}

/// Remove one response whose boundary is knowable now.
///
/// `Content-Length` makes provisional responses deliverable before FIN. Without it, FIN is the
/// mapping's body delimiter, so the response deliberately remains buffered until `end`.
fn take_response(
    pending: &mut BytesMut,
    end: bool,
    limits: &Limits,
) -> std::result::Result<Option<Message>, String> {
    let Some(head_end) = pending.windows(4).position(|window| window == b"\r\n\r\n") else {
        if pending.len() > limits.max_message_bytes || end && !pending.is_empty() {
            return Err("response has no complete header section".to_owned());
        }
        return Ok(None);
    };
    let body_start = head_end
        .checked_add(4)
        .ok_or_else(|| "response length overflow".to_owned())?;
    let header = pending
        .get(..head_end)
        .ok_or_else(|| "response header length overflow".to_owned())?;
    let frame_len = match declared_content_length(header)? {
        Some(declared) => body_start
            .checked_add(declared)
            .ok_or_else(|| "response length overflow".to_owned())?,
        None if end => pending.len(),
        None => {
            if pending.len() > limits.max_message_bytes {
                return Err("response exceeds the configured message limit".to_owned());
            }
            return Ok(None);
        }
    };
    if frame_len > limits.max_message_bytes {
        return Err("response exceeds the configured message limit".to_owned());
    }
    if pending.len() < frame_len {
        if end {
            return Err("response body ended before Content-Length".to_owned());
        }
        return Ok(None);
    }
    parse_one(pending.split_to(frame_len).freeze(), limits).map(Some)
}

fn declared_content_length(head: &[u8]) -> std::result::Result<Option<usize>, String> {
    let mut lines = head.split(|byte| *byte == b'\n');
    let _start_line = lines.next();
    let mut value: Option<Vec<u8>> = None;
    let mut continuing_length = false;
    for raw_line in lines {
        let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        if line.first().is_some_and(u8::is_ascii_whitespace) {
            if continuing_length {
                let stored = value
                    .as_mut()
                    .ok_or_else(|| "missing Content-Length value".to_owned())?;
                stored.push(b' ');
                stored.extend_from_slice(trim_ascii(line));
            }
            continue;
        }
        continuing_length = false;
        let mut parts = line.splitn(2, |byte| *byte == b':');
        let name = parts.next().unwrap_or_default();
        let Some(raw_value) = parts.next() else {
            continue;
        };
        let name = trim_ascii(name);
        if name.eq_ignore_ascii_case(b"content-length") || name.eq_ignore_ascii_case(b"l") {
            if value.is_some() {
                return Err("repeated Content-Length".to_owned());
            }
            value = Some(trim_ascii(raw_value).to_vec());
            continuing_length = true;
        }
    }
    value
        .map(|value| {
            std::str::from_utf8(&value)
                .map_err(|error| error.to_string())?
                .parse::<usize>()
                .map_err(|error| error.to_string())
        })
        .transpose()
}

fn trim_ascii(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = bytes.get(1..).unwrap_or_default();
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = bytes
            .get(..bytes.len().saturating_sub(1))
            .unwrap_or_default();
    }
    bytes
}

/// Parse the stream's one message and reject a declared length that does not consume it exactly.
#[allow(
    clippy::needless_pass_by_value,
    reason = "the parsed message retains views into this owned Bytes allocation"
)]
fn parse_one(bytes: Bytes, limits: &Limits) -> std::result::Result<Message, String> {
    let head_end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "no header terminator".to_owned())?;
    let message = parse_datagram(bytes.clone(), limits).map_err(|error| error.to_string())?;
    if let Some(value) = message
        .headers()
        .value(&sipx_sip::HeaderName::ContentLength)
    {
        let declared = std::str::from_utf8(&value)
            .map_err(|error| error.to_string())?
            .trim()
            .parse::<usize>()
            .map_err(|error| error.to_string())?;
        let actual = bytes.len().saturating_sub(head_end + 4);
        if declared != actual {
            return Err(format!(
                "Content-Length {declared} does not match {actual} stream bytes"
            ));
        }
    }
    Ok(message)
}
