//! Transport errors.

use thiserror::Error;

/// What can go wrong in the transport layer.
#[derive(Debug, Error)]
pub enum Error {
    /// A socket operation failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The endpoint loop has stopped.
    #[error("the endpoint has shut down")]
    EndpointClosed,
    /// A request could not be sent because it has no usable `Via`, so no transaction could be
    /// keyed on it and no response could ever be matched.
    #[error("the request has no usable Via")]
    NoVia,
    /// A message could not be built.
    #[error("build: {0}")]
    Build(#[from] sipx_sip::error::BuildError),
    /// A URI that resolved to no usable candidate (RFC 3263).
    #[error("no usable candidate for {}", String::from_utf8_lossy(.0))]
    Unresolvable(Vec<u8>),
    /// A transport that is declared but not yet implemented.
    #[error("the {0} transport is not implemented yet")]
    UnsupportedTransport(&'static str),
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
