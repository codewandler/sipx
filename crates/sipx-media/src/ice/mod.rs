//! ICE (RFC 8445): finding a path for media when both ends are behind NATs.
//!
//! The SDP half of ICE — the `a=candidate`, `a=ice-ufrag` and `a=ice-pwd` grammar of RFC 8839 §5
//! — is [`sipx_sdp::ice`], because it is pure parsing and belongs where no clock and no socket
//! can reach it. What lives here is everything else: the STUN profile RFC 8445 §7 runs over the
//! media port, and, as the epic lands, the agent that drives it.
//!
//! The normative document is [`docs/specs/ice.md`], written before any of this code; §11 is the
//! STUN profile [`stun`] implements.
//!
//! [`docs/specs/ice.md`]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md

pub mod agent;
pub mod candidate;
pub mod checklist;
pub mod stun;
pub mod timing;

pub use agent::{Agent, Config, Input, Output, Timer};
pub use candidate::{Gathered, LocalBase, LocalCandidate, RemoteCandidate};
pub use checklist::{ChecklistState, PairState, Role};
pub use timing::Timers;
