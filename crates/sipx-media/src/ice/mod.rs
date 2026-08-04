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
//! **Supported** (`A-8`): calls reach the gathering and selected-pair driver through
//! `sipx_call::MediaPolicy`. Breaking changes therefore carry a migration note even while sipx is
//! pre-1.0; new enum variants and fields may still be added.
//!

pub mod agent;
pub mod candidate;
pub mod checklist;
pub(crate) mod driver;
pub mod gather;
pub mod negotiate;
pub mod stun;
pub mod timing;

pub use agent::{Agent, Config, Input, Output, Timer};
pub use candidate::{Gathered, LocalBase, LocalCandidate, RemoteCandidate};
pub use checklist::{ChecklistState, PairState, Role};
pub use driver::{IcePath, Local};
pub use gather::{Gathering, LocalDescription};
pub use negotiate::{ICE_MISMATCH, Negotiation, negotiate};
pub use timing::Timers;
