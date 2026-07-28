//! SIP user agent: registration, authentication, and answering what arrives.
//!
//! This crate sits on `sipx-transport` and turns transactions into the things a phone or a
//! service actually does. Digest authentication and registration leases live here because
//! both are about *state over time* rather than about a single message, which is what
//! separates a user agent from a transaction layer.
//!
//! Dialogs and calls are the next layer up, in `sipx-call`.

pub mod agent;
pub mod auth;
pub mod challenge;
pub mod error;
pub mod flows;
pub mod outbound;
pub mod registrar;

pub use agent::{Config, Flow, UserAgent};
pub use auth::{Algorithm, Challenge, Credentials};
pub use challenge::{Authenticator, Presented, Reason, Verdict};
pub use error::{Error, Result};
pub use flows::{Attempt, Flows};
pub use outbound::{InstanceId, Keepalive, Power, RegId};
pub use registrar::{Lease, Outcome, PathSet, Registered, Registration, ServiceRoute};
