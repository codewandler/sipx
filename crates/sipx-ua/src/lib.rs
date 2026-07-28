//! SIP user agent: registration, authentication, and answering what arrives.
//!
//! This crate sits on `sipx-transport` and turns transactions into the things a phone or a
//! service actually does. Digest authentication and registration leases live here because
//! both are about *state over time* rather than about a single message, which is what
//! separates a user agent from a transaction layer.
//!
//! Dialogs and calls are the next layer up, in `sipx-call`.

//! # Without a runtime
//!
//! Digest is hashing and header text, and a caller whose decision logic touches no IO must be able
//! to use it without linking one. `default-features = false` drops the `runtime` feature and with
//! it the modules that drive a socket — `agent`, `flows`, and the error type that wraps a transport
//! failure — leaving `auth`, `challenge`, `outbound` and `registrar`. The alternative for such a
//! caller is to write digest a second time, and two implementations of one algorithm eventually
//! disagree about who is authenticated.

#[cfg(feature = "runtime")]
pub mod agent;
pub mod auth;
pub mod challenge;
#[cfg(feature = "runtime")]
pub mod error;
#[cfg(feature = "runtime")]
pub mod flows;
pub mod outbound;
pub mod registrar;
pub mod subscribe;

#[cfg(feature = "runtime")]
pub use agent::{Config, Flow, UserAgent};
pub use auth::{Algorithm, Challenge, Credentials};
pub use challenge::{Authenticator, Presented, Reason, Verdict};
#[cfg(feature = "runtime")]
pub use error::{Error, Result};
#[cfg(feature = "runtime")]
pub use flows::{Attempt, Flows};
pub use outbound::{InstanceId, Keepalive, Power, RegId};
pub use registrar::{Lease, Outcome, PathSet, Registered, Registration, ServiceRoute};
