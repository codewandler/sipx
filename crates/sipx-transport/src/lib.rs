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
//!
//! # Stability
//!
//! sipx is pre-1.0, so **neither word below means frozen**. `1.0.0` is what freezes an API, and its
//! predicates are in `docs/roadmap.md`. Until then:
//!
//! - **Supported** — meant to be depended on. Breaking changes get a `CHANGELOG.md` entry saying what
//!   to do instead. New enum variants and new struct fields may still appear in a minor release, so a
//!   downstream `match` should carry a `_` arm.
//! - **Experimental** — may change shape or be removed without a migration note. Depend on it only if
//!   you are prepared to follow it.
//!
//!
//! **Supported.** UDP, TCP, TLS and WebSocket are all in the default feature set and all carry a call.
//! `respond` guarantees the response is on the wire before it returns — see
//! `docs/designs/sip-transport.md`, which records that as a guarantee rather than an internal detail.
//!
//! **Experimental.** QUIC is enabled by default so its feature-off build is continuously checked,
//! but its SIP mapping is a sipx specification rather than an RFC. Its API and wire choices may
//! change if a standard mapping is published; see `docs/specs/sip-quic.md`.

pub mod capture;
pub mod counters;
#[cfg(feature = "dns")]
pub mod dns;
pub mod endpoint;
pub mod error;
pub mod nat;
pub mod overload;
pub mod policy;
#[cfg(feature = "quic")]
pub mod quic;
pub mod resolve;
pub mod stun;
pub mod target;
pub mod tcp;
pub mod timers;
#[cfg(feature = "tls")]
pub mod tls;
#[cfg(feature = "ws")]
pub mod ws;

pub use capture::{CaptureConfig, Direction, HepConfig};
pub use counters::{
    CaptureCounts, Counters, DiscardCounts, ShedCounts, TimeoutCounts, TransportCounts,
    UnsentCounts,
};
pub use endpoint::{
    CancelInviteOutcome, CancelTransactionOutcome, CleartextTransports, Config, Handle,
    InProcessEndpoint, InProcessPair, Incoming, InviteCancellation, Responses, Unmatched, bind,
    in_process_pair, new_branch,
};
pub use error::{Error, Result};
pub use overload::{OverloadConfig, OverloadFeedback, RequestCategory};
pub use policy::{
    ConnectionId, ConnectionObservation, ConnectionState, EndpointObservation, MessageDirection,
    MessageObservation, RequestPolicy, RequestPolicyDecision, RequestPolicyRef, SourcePrefix,
    TransactionClass,
};
pub use resolve::{Naptr, Resolver, Srv, resolve};
pub use stun::Reply as StunReply;
pub use target::{ConnectionKey, Target, TransportKind};
pub use tcp::{Pool, PoolConfig};
