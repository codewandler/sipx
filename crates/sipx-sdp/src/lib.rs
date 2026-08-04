//! SDP session descriptions (RFC 8866) and offer/answer negotiation (RFC 3264).
//!
//! Two things shape this crate.
//!
//! **Unknown lines survive.** SDP is extended constantly, and an element that drops what it
//! does not understand breaks features it has never heard of. Parsing keeps every line; the
//! typed accessors are a view over them, not a replacement.
//!
//! **Negotiation is a pure function.** [`answer()`] takes an offer and a set of capabilities and
//! returns an answer — no sockets, no clock, no shared mutable session object. The rules in
//! RFC 3264 are full of cases that are awkward to reach through a live call (a stream with no
//! common codec, a `sendonly` that must become `recvonly`, a dynamic payload type that means
//! different things at each end) and they are all one function call away here.
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
//! **Supported.** This includes `with_dtls_srtp` and the ICE attribute helpers: `sipx-call`
//! consumes both for explicit media policy in dial and answer roles (`M-27`, `M-28`).

pub mod answer;
pub mod crypto;
pub mod fingerprint;
pub mod ice;
pub mod parse;
pub mod rtpmap;
pub mod session;

pub use answer::{Capabilities, answer, negotiate_direction};
pub use ice::{
    Candidate, CandidateType, ComponentId, Credentials, Foundation, Pacing, Priority,
    RelatedAddress, RemoteCandidate, Transport,
};
pub use parse::parse;
pub use session::{
    Address, Attribute, Connection, Direction, MediaDescription, Origin, SessionDescription, Timing,
};

/// What can go wrong reading SDP.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SdpError {
    /// A line was not `x=value`.
    #[error("malformed line: {0}")]
    MalformedLine(String),
    /// A required line was missing.
    #[error("missing the {0} line")]
    Missing(&'static str),
    /// A field did not parse.
    #[error("invalid {field}: {value}")]
    Invalid {
        /// Which field.
        field: &'static str,
        /// What it contained.
        value: String,
    },
}

/// An SDP result.
pub type Result<T> = std::result::Result<T, SdpError>;
