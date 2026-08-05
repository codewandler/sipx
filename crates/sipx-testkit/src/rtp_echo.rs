//! A bounded RTP/PCMU echo peer for downstream media tests.
//!
//! This is a diagnostic fixture over the public packet and codec APIs, not a SIP user agent or a
//! production media service. One [`RtpEcho`] owns one UDP socket and [`RtpEcho::run`] spawns no
//! task, so completion, error and cancellation all have the same cleanup path: drop the future and
//! the socket is released.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::time::Duration;

use bytes::Bytes;
use sipx_audio::g711;
use sipx_rtp::{Packet, RtpError};
use thiserror::Error;
use tokio::net::UdpSocket;

/// Largest RTP datagram this telephony fixture admits.
pub const MAX_DATAGRAM_BYTES: usize = 2048;

const ECHO_SSRC: u32 = 0x5350_5854;
const PCMU_PAYLOAD_TYPE: u8 = 0;

/// A bounded RTP echo operation that could not be performed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EchoError {
    /// A configuration value cannot describe a finite, reachable fixture.
    #[error("invalid {field}: {reason}")]
    InvalidConfig {
        /// The rejected field.
        field: &'static str,
        /// Why it was rejected.
        reason: &'static str,
    },
    /// Binding, receiving or sending on the sole UDP socket failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A datagram from the configured peer was not a valid RTP packet.
    #[error(transparent)]
    Rtp(#[from] RtpError),
    /// A source other than the configured test peer sent a datagram.
    #[error("received RTP from {actual}, expected {expected}")]
    UnexpectedPeer {
        /// The sole admitted peer.
        expected: SocketAddr,
        /// The source that actually sent the datagram.
        actual: SocketAddr,
    },
    /// The fixture carries PCMU only.
    #[error("RTP payload type {0} is unsupported; the echo fixture accepts PCMU payload type 0")]
    UnsupportedPayloadType(u8),
    /// A datagram could not be admitted without truncating it.
    #[error("RTP datagram exceeds the {limit}-byte fixture limit")]
    DatagramTooLarge {
        /// Maximum admitted datagram length.
        limit: usize,
    },
    /// UDP reported a partial datagram send.
    #[error("UDP sent {sent} of {expected} echo bytes")]
    PartialSend {
        /// Bytes reported sent.
        sent: usize,
        /// Complete encoded packet length.
        expected: usize,
    },
    /// The whole-run deadline elapsed before every configured packet was echoed.
    #[error("echoed {received} of {expected} packets before the {within:?} run bound elapsed")]
    TimedOut {
        /// Packets successfully echoed.
        received: usize,
        /// Finite configured packet count.
        expected: usize,
        /// Whole-run failure bound.
        within: Duration,
    },
}

/// Explicit, finite configuration for one RTP echo run.
#[derive(Debug, Clone, Copy)]
pub struct EchoConfig {
    bind: SocketAddr,
    peer: SocketAddr,
    packets: NonZeroUsize,
    within: Duration,
}

impl EchoConfig {
    /// Validate one finite RTP/PCMU echo run.
    ///
    /// Port zero is accepted for `bind`, which is useful in a test that reads [`RtpEcho::local_addr`]
    /// before it starts its peer. The peer must be a concrete unicast destination with a non-zero
    /// port, and both addresses must use the same IP family.
    pub fn new(
        bind: SocketAddr,
        peer: SocketAddr,
        packets: NonZeroUsize,
        within: Duration,
    ) -> Result<Self, EchoError> {
        let broadcast = peer.ip() == IpAddr::V4(Ipv4Addr::BROADCAST);
        if peer.ip().is_unspecified() || peer.ip().is_multicast() || broadcast || peer.port() == 0 {
            return Err(EchoError::InvalidConfig {
                field: "peer",
                reason: "must be a concrete unicast address with a non-zero port",
            });
        }
        if bind.is_ipv4() != peer.is_ipv4() {
            return Err(EchoError::InvalidConfig {
                field: "peer",
                reason: "must use the bind address family",
            });
        }
        if within.is_zero() {
            return Err(EchoError::InvalidConfig {
                field: "within",
                reason: "must be greater than zero",
            });
        }
        Ok(Self {
            bind,
            peer,
            packets,
            within,
        })
    }

    /// Local address requested for the sole UDP socket.
    #[must_use]
    pub const fn bind_addr(self) -> SocketAddr {
        self.bind
    }

    /// Sole admitted RTP source and echo destination.
    #[must_use]
    pub const fn peer(self) -> SocketAddr {
        self.peer
    }

    /// Exact number of valid packets the run echoes.
    #[must_use]
    pub const fn packets(self) -> NonZeroUsize {
        self.packets
    }

    /// Whole-run failure bound.
    #[must_use]
    pub const fn within(self) -> Duration {
        self.within
    }
}

/// A bound, not-yet-running RTP echo fixture.
#[derive(Debug)]
pub struct RtpEcho {
    socket: UdpSocket,
    local_addr: SocketAddr,
    config: EchoConfig,
}

/// What one finite echo run completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EchoReport {
    /// RTP packets decoded and echoed.
    pub packets: usize,
    /// Decoded PCMU samples carried by those packets.
    pub samples: usize,
}

impl RtpEcho {
    /// Bind the fixture's sole UDP socket.
    pub async fn bind(config: EchoConfig) -> Result<Self, EchoError> {
        let socket = UdpSocket::bind(config.bind).await?;
        let local_addr = socket.local_addr()?;
        Ok(Self {
            socket,
            local_addr,
            config,
        })
    }

    /// The actual bound address, including the OS-selected port when configuration used port zero.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Echo exactly the configured number of valid PCMU packets within the whole-run deadline.
    ///
    /// This consumes the fixture and spawns no task. Cancelling the future therefore drops the
    /// sole socket immediately; completion and error take the same ownership path.
    pub async fn run(self) -> Result<EchoReport, EchoError> {
        let expected = self.config.packets.get();
        let within = self.config.within;
        let deadline = tokio::time::Instant::now() + within;
        let mut buffer = [0_u8; MAX_DATAGRAM_BYTES + 1];
        let mut packets = 0_usize;
        let mut samples = 0_usize;
        let mut sequence = 0_u16;
        let mut timestamp = 0_u32;

        while packets < expected {
            let received = tokio::time::timeout_at(
                deadline, // failure bound: maximum lifetime of one complete echo run
                self.socket.recv_from(&mut buffer),
            )
            .await;
            let (length, source) = match received {
                Ok(result) => result?,
                Err(_) => {
                    return Err(EchoError::TimedOut {
                        received: packets,
                        expected,
                        within,
                    });
                }
            };
            if source != self.config.peer {
                return Err(EchoError::UnexpectedPeer {
                    expected: self.config.peer,
                    actual: source,
                });
            }
            if length > MAX_DATAGRAM_BYTES {
                return Err(EchoError::DatagramTooLarge {
                    limit: MAX_DATAGRAM_BYTES,
                });
            }
            let input =
                Packet::decode(&Bytes::copy_from_slice(buffer.get(..length).unwrap_or(&[])))?;
            if input.payload_type != PCMU_PAYLOAD_TYPE {
                return Err(EchoError::UnsupportedPayloadType(input.payload_type));
            }

            let decoded = g711::ulaw_decode_all(&input.payload);
            let sample_count = decoded.len();
            let output = Packet::new(
                PCMU_PAYLOAD_TYPE,
                sequence,
                timestamp,
                ECHO_SSRC,
                Bytes::from(g711::ulaw_encode_all(&decoded)),
            )
            .encode();
            let sent = match tokio::time::timeout_at(
                deadline, // failure bound: sending is inside the same whole-run deadline
                self.socket.send_to(&output, self.config.peer),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => {
                    return Err(EchoError::TimedOut {
                        received: packets,
                        expected,
                        within,
                    });
                }
            };
            if sent != output.len() {
                return Err(EchoError::PartialSend {
                    sent,
                    expected: output.len(),
                });
            }

            packets = packets.saturating_add(1);
            samples = samples.saturating_add(sample_count);
            sequence = sequence.wrapping_add(1);
            timestamp = timestamp.wrapping_add(u32::try_from(sample_count).unwrap_or(u32::MAX));
        }
        Ok(EchoReport { packets, samples })
    }
}
