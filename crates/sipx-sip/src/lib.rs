//! Sans-IO SIP core.
//!
//! This crate implements SIP (RFC 3261) as pure state machines: message parsing and
//! serialization, the client and server transaction FSMs, and dialog identity and state.
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

pub mod error;
mod escape;
pub mod headers;
pub mod message;
pub mod name;
pub mod params;
pub mod parser;
pub mod uri;
pub mod validate;

pub use error::{HeaderError, UriError};
pub use headers::{Address, CSeq, CallId, Via};
pub use message::{
    Header, Headers, Message, Method, Request, Response, StatusCode, TypedHeader, Version,
};
pub use name::HeaderName;
pub use params::{Param, Params};
pub use parser::{Limits, StreamParser, parse_datagram};
pub use uri::{Host, Scheme, Uri};
pub use validate::{Finding, validate, validate_request, validate_response};
