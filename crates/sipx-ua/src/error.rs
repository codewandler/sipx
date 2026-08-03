//! User agent errors.

use thiserror::Error;

/// What can go wrong in the user agent.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The transport failed.
    #[error("transport: {0}")]
    Transport(#[from] sipx_transport::Error),
    /// A message could not be built.
    #[error("build: {0}")]
    Build(#[from] sipx_sip::error::BuildError),
    /// The transaction ended without a final response — a timeout or a transport failure.
    #[error("no final response")]
    NoResponse,
    /// The server challenged and no credentials were configured.
    #[error("the server requires credentials and none were configured")]
    CredentialsRequired,
    /// The server challenged again after credentials were supplied, and did not say the nonce
    /// was stale. Retrying would be guessing at a password.
    #[error("authentication failed")]
    AuthenticationFailed,
    /// The flow's reflexive address changed, so the flow has failed (RFC 5626 §4.4.2).
    ///
    /// Not a transport error: the socket works. The NAT rebound, so the mapping the registrar
    /// holds for this flow no longer reaches it, and §4.4.2 requires the UA to treat that as a
    /// failure and re-establish rather than carry on pinging an address nothing routes to.
    #[error("the flow's reflexive address changed from {previous} to {current}")]
    FlowRebound {
        /// The address the previous keep-alive reported.
        previous: std::net::SocketAddr,
        /// The address this one did.
        current: std::net::SocketAddr,
    },
    /// More flows were added than `reg-id` can number (RFC 5626 §4.2 caps it at 2^31 - 1).
    #[error("too many flows: reg-id cannot number more than 2^31 - 1 of them")]
    TooManyFlows,
    /// The registrar answered 555: it does not support the push notification service the
    /// `Contact` named (RFC 8599 §8.1).
    ///
    /// Distinct from [`Error::Rejected`] because retrying cannot help. Every attempt naming this
    /// push service will be refused the same way, and a client that treats it as a transient
    /// failure stays unreachable while looking busy.
    #[error("the registrar does not support the push notification service named: 555 {reason}")]
    PushNotSupported {
        /// The reason phrase the registrar sent.
        reason: String,
    },
    /// The server refused.
    #[error("rejected: {status} {reason}")]
    Rejected {
        /// The status code.
        status: u16,
        /// Its reason phrase.
        reason: String,
    },
}

/// A user agent result.
pub type Result<T> = std::result::Result<T, Error>;
