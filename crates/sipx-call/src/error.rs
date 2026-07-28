//! Call errors.

use thiserror::Error;

/// What can go wrong establishing or running a call.
#[derive(Debug, Error)]
pub enum Error {
    /// A socket failed.
    #[error("io: {0}")]
    Io(#[source] std::io::Error),
    /// The transport failed.
    #[error("transport: {0}")]
    Transport(#[from] sipx_transport::Error),
    /// A message could not be built.
    #[error("build: {0}")]
    Build(#[from] sipx_sip::error::BuildError),
    /// The SDP could not be read.
    #[error("sdp: {0}")]
    Sdp(String),
    /// The INVITE got no final response.
    #[error("no final response to the INVITE")]
    NoResponse,
    /// The far end refused.
    #[error("rejected: {status} {reason}")]
    Rejected {
        /// The status code.
        status: u16,
        /// Its reason phrase.
        reason: String,
    },
    /// A 2xx that established no dialog — no `To` tag, or no `Contact` to send to.
    #[error("the response established no dialog")]
    NoDialog,
    /// Negotiation produced no usable audio stream.
    #[error("no codec in common")]
    NoCommonCodec,
}

/// A call result.
pub type Result<T> = std::result::Result<T, Error>;
