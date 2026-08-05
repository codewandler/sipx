//! Transport errors.

use thiserror::Error;

/// What can go wrong in the transport layer.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Endpoint configuration cannot create a bounded, live runtime.
    #[error("invalid endpoint configuration `{field}`: {reason}")]
    InvalidConfig {
        /// The public configuration field that is invalid.
        field: &'static str,
        /// Its required range.
        reason: &'static str,
    },
    /// A socket operation failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The endpoint loop has stopped.
    #[error("the endpoint has shut down")]
    EndpointClosed,
    /// An in-process endpoint was constructed without an entered Tokio runtime.
    #[error("an entered Tokio runtime is required for the in-process endpoint")]
    RuntimeUnavailable,
    /// The next hop asked this endpoint to reduce traffic and this request was not admitted.
    #[error("request rejected by overload control for {peer}")]
    Overloaded {
        /// The downstream server whose active report caused the rejection.
        peer: std::net::SocketAddr,
    },
    /// A request could not be sent because it has no usable `Via`, so no transaction could be
    /// keyed on it and no response could ever be matched.
    #[error("the request has no usable Via")]
    NoVia,
    /// A message could not be built.
    #[error("build: {0}")]
    Build(#[from] sipx_sip::error::BuildError),
    /// TLS could not be established or verified.
    ///
    /// A `sips:` request that reaches this has failed. There is deliberately no path from here
    /// to a cleartext retry: a downgrade would defeat exactly what the scheme asked for.
    #[cfg(feature = "tls")]
    #[error("tls: {0}")]
    Tls(#[from] crate::tls::TlsError),
    /// QUIC authentication, negotiation, or connection failure.
    #[cfg(feature = "quic")]
    #[error("quic: {0}")]
    Quic(#[from] crate::quic::QuicError),
    /// A response was given for a transaction that no longer exists.
    ///
    /// Almost always means the application took longer to answer than
    /// [`crate::Config::unanswered_limit`] allows. Reported rather than swallowed: an
    /// application told its 200 OK went out, when it did not, believes a call is up while the
    /// caller has already timed out.
    #[error("no such transaction; it was abandoned or has already ended")]
    NoTransaction,
    /// A keep-alive was answered with a STUN Binding Error Response (RFC 5626 §4.4.2).
    ///
    /// §4.4.2 says the flow "is considered failed" — a *refused* keep-alive is a stronger signal
    /// than an unanswered one, since something is there and it does not want this flow.
    #[error("the keep-alive was refused; the flow has failed")]
    KeepaliveRefused,
    /// A keep-alive went unanswered (RFC 5626 §4.4.1, §4.4.2).
    ///
    /// §4.4.1: "If a pong is not received within 10 seconds after sending a ping ... then the
    /// client MUST treat the flow as failed."
    #[error("the keep-alive went unanswered; the flow has failed")]
    KeepaliveUnanswered,
    /// The connection a keep-alive was sent on closed before it was answered.
    #[error("the connection closed")]
    ConnectionClosed,
    /// A URI that resolved to no usable candidate (RFC 3263).
    #[error("no usable candidate for {}", String::from_utf8_lossy(.0))]
    Unresolvable(Vec<u8>),
    /// A transport that is declared but not yet implemented.
    #[error("the {0} transport is not implemented yet")]
    UnsupportedTransport(&'static str),
    /// Every configured live connection slot is still occupied.
    #[error("the connection pool's {max} live slots are occupied")]
    ConnectionCapacity {
        /// The configured live-task limit.
        max: usize,
    },
    /// A capture file could not be opened (`docs/specs/sip-transport.md` §13).
    ///
    /// Reported from `bind` rather than swallowed, because the alternative is an endpoint that
    /// starts, appears to be recording, and writes nothing — the same failure as a silent discard,
    /// one level up. The path is named because a permission or directory mistake is the usual cause.
    #[error("the capture at {path} could not be opened: {source}")]
    Capture {
        /// Where the capture was to be written.
        path: String,
        /// Why it could not be.
        source: std::io::Error,
    },
    /// A datagram larger than the path MTU on an unreliable transport (RFC 3261 §18.1.1).
    ///
    /// Named rather than truncated: a truncated SIP message is a security problem, not a
    /// degraded one.
    #[error("message of {size} bytes exceeds the {mtu} byte datagram limit")]
    TooLarge {
        /// How big the message is.
        size: usize,
        /// The configured limit.
        mtu: usize,
    },
}

/// A transport result.
pub type Result<T> = std::result::Result<T, Error>;
