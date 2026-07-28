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
    /// The caller gave up before the far end answered, and cancelled the invitation.
    ///
    /// Distinct from a rejection: nobody refused the call, we stopped waiting for it.
    #[error("no answer within {0:?}; the invitation was cancelled")]
    Cancelled(std::time::Duration),
    /// An INVITE asked to replace a dialog it did not name, or named one this is not.
    ///
    /// Deliberately one error for both. Telling a caller "the Call-ID matched but the tags did
    /// not" would be telling them how far their guess got.
    #[error("the Replaces header names no dialog we have")]
    NoReplaces,
    /// A transfer was accepted or refused when none had been asked for.
    #[error("no transfer has been requested on this call")]
    NoReferral,
    /// Negotiation produced no usable audio stream.
    #[error("no codec in common")]
    NoCommonCodec,
    /// A `422` refused the session interval we asked for (RFC 4028 §6), naming its own minimum.
    ///
    /// Carries the minimum rather than folding into [`Self::Rejected`] because the whole point
    /// of a 422 is that it is retryable, and only the value it carries makes the retry possible.
    #[error("the far end requires a session interval of at least {0:?}")]
    IntervalTooBrief(std::time::Duration),
    /// The far end never refreshed the session, so it was torn down locally (RFC 4028 §10).
    #[error("the session expired without a refresh")]
    SessionExpired,
}

/// A call result.
pub type Result<T> = std::result::Result<T, Error>;
