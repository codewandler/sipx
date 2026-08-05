//! Sans-IO SIP core.
//!
//! This crate implements SIP (RFC 3261) as pure state machines: message parsing and
//! serialization, and the client and server transaction FSMs.
//! It performs **no I/O**, spawns no tasks, and reads no clock. Time enters as a fired-timer
//! input and leaves as a set-timer output; bytes enter as received data and leave as data to
//! send. Async transports live in `sipx-transport`.
//!
//! That separation is deliberate. Every hard part of SIP — retransmission timing, transaction
//! matching, malformed input handling — becomes testable without sockets and fuzzable without
//! a runtime.
//!
//! # Reading hostile input
//!
//! Everything here parses data from the network, so nothing here panics. `unsafe` is
//! forbidden, indexing is checked, and every fallible operation returns a `Result` whose error
//! names the specific fault — the transaction layer picks a response status from it.
//!
//! Parsing is also *lazy and layered*: a message that frames correctly parses even if one of
//! its headers is malformed, because a proxy must be able to forward what it cannot itself
//! interpret. See `docs/specs/sip-message.md`.
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
//! **Supported.** Parsing, serialisation and the §17 transaction machines are the most heavily tested
//! surface in the workspace and are what everything above is built on.
//!
//! The public error enums are `#[non_exhaustive]`. New typed parse failures may be added without
//! breaking downstream callers, so a `match` over one carries a `_` arm.

pub mod auth;
pub mod build;
pub mod error;
mod escape;
pub mod event;
pub mod gruu;
pub mod headers;
pub mod identity;
pub mod message;
pub mod name;
pub mod params;
pub mod parser;
pub mod push;
pub mod rel;
pub mod session;
pub mod transaction;
pub mod update;
pub mod uri;
pub mod validate;

pub use build::{RequestBuilder, ResponseBuilder};
pub use error::{AddressEditError, BuildError, HeaderError, UriError};
pub use headers::{
    Address, CSeq, CallId, HistoryEntry, HistoryIndex, HistoryInfo, IgnoredIdentity,
    IgnoredIdentityReason, PAssertedIdentity, PAssertedIdentityList, PPreferredIdentity,
    PPreferredIdentityList, Privacy, PrivacyList, PrivacyValue, Reason, ReasonValue, TargetChange,
    TargetChangeKind, Via,
};
pub use message::{
    Header, Headers, Message, Method, Request, Response, StatusCode, TypedHeader, Version,
};
pub use name::HeaderName;
pub use params::{Param, Params};
pub use parser::{Limits, StreamParser, parse_datagram};
pub use transaction::{
    ClientTransaction, Output, Reliability, ServerTransaction, Timer, Timers, TransactionKey,
    TransactionLayer, TuEvent,
};
pub use uri::{Host, HostName, Scheme, TelUriParts, Uri, UriTransport, UriTransportError};
pub use validate::{Finding, validate, validate_request, validate_response};
