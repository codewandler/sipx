//! Async SIP transports.
//!
//! This crate is the driver for the sans-IO core in [`sipx_sip`]: it owns sockets,
//! connections and timers, feeds received bytes in as inputs, and performs the outputs the
//! core asks for. It also implements the parts of real networks the RFCs leave to the
//! implementation — `received` and `rport` for NAT, connection reuse, target resolution.
//!
//! The shape of it is one event loop per endpoint, owning everything mutable. Nothing in the
//! signalling path takes a lock, and no transaction is reachable from two tasks.
//!
//! See `docs/specs/sip-transport.md`.

pub mod endpoint;
pub mod error;
pub mod nat;
pub mod target;
pub mod timers;

pub use endpoint::{Config, Handle, Incoming, Responses, bind, new_branch};
pub use error::{Error, Result};
pub use target::{Target, TransportKind};
