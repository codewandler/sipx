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
pub mod error;
pub mod registrar;

pub use agent::{Config, UserAgent};
pub use auth::{Algorithm, Challenge, Credentials};
pub use error::{Error, Result};
pub use registrar::{Lease, Outcome, PathSet, Registration};
