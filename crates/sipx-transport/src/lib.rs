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

#[cfg(feature = "dns")]
pub mod dns;
pub mod endpoint;
pub mod error;
pub mod nat;
pub mod resolve;
pub mod target;
pub mod tcp;
pub mod timers;
#[cfg(feature = "tls")]
pub mod tls;
#[cfg(feature = "ws")]
pub mod ws;

pub use endpoint::{Config, Handle, Incoming, Responses, bind, new_branch};
pub use error::{Error, Result};
pub use resolve::{Naptr, Resolver, Srv, resolve};
pub use target::{ConnectionKey, Target, TransportKind};
pub use tcp::{Pool, PoolConfig};
