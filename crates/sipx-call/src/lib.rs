//! Calls: dialogs, INVITE with SDP offer/answer, and the media that results.
//!
//! This is the layer where the signalling and the media stacks meet. The join is narrower than
//! it looks: SDP negotiation decides an address, a port and a codec, and everything else about
//! media follows from those three.
//!
//! The ordering constraint worth knowing is that an SDP offer has to name the port audio will
//! arrive on, and only a bound socket knows that port. So the media socket is bound *before*
//! the INVITE is sent, not after the answer comes back.
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
//! **Supported**: the call lifecycle — dial, answer, early dialogs, hold and resume, both transfer
//! flavours, DTMF, playback, recording, session timers.
//!
//! Absent rather than experimental, so that nobody looks for it: **multi-party**. Bridging and
//! conferencing exist in `sipx-media` over sessions you own, and this crate does not expose its
//! `MediaSession`, so two `Call`s cannot be joined (`C-6`). A call also cannot answer an
//! authentication challenge (`S-28`) or select a codec other than G.711 (`M-30`).
//!
//! [`Error`] is **not** `#[non_exhaustive]` and went from 13 variants to 16 in one release, breaking
//! every downstream exhaustive `match`. Treat it as though it were: carry a `_` arm.

pub mod call;
pub mod dialog;
pub mod dispatch;
pub mod error;
pub mod event;
pub mod rel;
pub mod transfer;
// Crate-private: every item in it is `pub(crate)`, and a `pub mod` whose contents are all
// private renders as an empty page in the API reference — a promise of surface that is not there.
mod update;

pub use call::{
    Call, Codecs, DialOptions, Dialing, answer, answer_early, answer_replacing,
    answer_replacing_with, answer_ringing, answer_ringing_with, answer_with, dial, dial_early,
    dial_once, serve,
};
pub use dialog::{Dialog, DialogId, Role};
pub use dispatch::{Calls, DispatchCounts, Dispatched, Dispatcher, Invitation};
pub use error::{Error, Result};
pub use event::{CallEvent, CallEvents, EndCause};
pub use rel::{Ringing, ring, ring_early, ring_early_with};
pub use transfer::{Referral, Replaces, Transfer, TransferState};
