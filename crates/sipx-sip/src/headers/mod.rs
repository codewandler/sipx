//! Typed header values.
//!
//! Headers are parsed on demand, never during message parsing. A message with a malformed
//! `CSeq` frames and forwards perfectly well; only a party that needs to *read* `CSeq` has a
//! problem, and it finds out by asking:
//!
//! ```no_run
//! # use sipx_sip::{Message, headers::CSeq};
//! # fn example(message: &Message) {
//! match message.headers().typed::<CSeq>() {
//!     None => { /* absent */ }
//!     Some(Err(_)) => { /* present and malformed — answer 400 */ }
//!     Some(Ok(cseq)) => { /* usable */ }
//! }
//! # }
//! ```
//!
//! The `Option<Result<..>>` is deliberate. Collapsing it is how implementations end up
//! treating a corrupt header as a missing one.
//!
//! Authentication headers are not here: they belong to the user-agent layer, which is where
//! anything that can act on a challenge lives.

pub mod address;
pub(crate) mod grammar;
pub mod history;
pub mod identity;
pub mod misc;
pub mod privacy;
pub mod via;

pub use address::{Address, Contact, ContactValue, From, RecordRoute, Route, To};
pub use grammar::HeaderParam;
pub use history::{
    HistoryEntry, HistoryIndex, HistoryInfo, Reason, ReasonValue, TargetChange, TargetChangeKind,
};
pub use identity::{
    IgnoredIdentity, IgnoredIdentityReason, PAssertedIdentity, PAssertedIdentityList,
    PPreferredIdentity, PPreferredIdentityList,
};
pub use misc::{
    Allow, CSeq, CallId, ContentLength, ContentType, Date, Expires, MaxForwards, ProxyRequire,
    Require, Supported, TokenList, Unsupported,
};
pub use privacy::{Privacy, PrivacyList, PrivacyValue};
pub use via::{
    BRANCH_MAGIC_COOKIE, OcParameter, OverloadAlgorithm, OverloadSequence, Via, ViaOverload,
    first_hop_end,
};
