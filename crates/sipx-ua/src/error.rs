//! User agent errors.

use thiserror::Error;

/// What can go wrong in the user agent.
#[derive(Debug, Error)]
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
