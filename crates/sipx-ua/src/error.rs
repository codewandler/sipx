//! User agent errors.

use thiserror::Error;

/// What can go wrong in the user agent.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The transport failed.
    #[error("transport: {0}")]
    Transport(#[from] sipx_transport::Error),
    /// The registrar's name could not be turned into an address (RFC 3263).
    ///
    /// Distinct from [`Error::Transport`] because nothing was dialled: no socket was opened and
    /// no packet was sent, so the fix is in the zone or in the resolver rather than at the
    /// address. `sipx_transport::destination::Error::kind` separates the two that a caller most
    /// needs apart — a zone with no answer, which is final, from a deadline, which says nothing
    /// about the name at all.
    #[error("resolution: {0}")]
    Resolution(#[from] sipx_transport::destination::Error),
    /// Every candidate a serial pass attempted failed to connect (RFC 3263 §4.3).
    ///
    /// The spec's `ConnectionFailed { attempted, last_error }`
    /// (`docs/specs/sip-target-resolution.md` §8). Distinct from [`Error::Transport`], which is
    /// one connection failing and says nothing about whether anything else was tried: the count
    /// is what separates a name that resolves to one dead host from a name every address behind
    /// which is unreachable. It renders as a transport failure still, because that is what it is
    /// and what a reader is looking for, with the pass named after it.
    ///
    /// `attempts.attempted()` is attempted-so-far — see
    /// [`sipx_transport::destination::Attempts`] for why a caller's deadline makes that the only
    /// honest reading, and why `resolved` travels beside it.
    #[error("transport: {source}, after attempting {attempts}")]
    ConnectionFailed {
        /// How far the pass got, and how far it could have gone.
        attempts: sipx_transport::destination::Attempts,
        /// The failure the last attempted candidate produced.
        #[source]
        source: sipx_transport::Error,
    },
    /// A message could not be built.
    #[error("build: {0}")]
    Build(#[from] sipx_sip::error::BuildError),
    /// The transaction ended without a final response — a timeout or a transport failure.
    #[error("no final response")]
    NoResponse,
    /// The caller's deadline for a whole registration attempt expired.
    ///
    /// Distinct from [`Error::NoResponse`], which is one client transaction reaching its own
    /// expiry: this is the budget a caller placed over the *attempt* — the initial REGISTER and
    /// any authentication retry — and it can fire while a transaction is still waiting. Both are
    /// "nothing answered in time", so a caller that only classifies may treat them alike; the
    /// distinction is what lets a report say which schedule the run was held to.
    #[error("the registration attempt did not complete within {}ms", limit.as_millis())]
    AttemptTimeout {
        /// The deadline the caller gave.
        limit: std::time::Duration,
    },
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
