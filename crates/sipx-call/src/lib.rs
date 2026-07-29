//! Calls: dialogs, INVITE with SDP offer/answer, and the media that results.
//!
//! This is the layer where the signalling and the media stacks meet. The join is narrower than
//! it looks: SDP negotiation decides an address, a port and a codec, and everything else about
//! media follows from those three.
//!
//! The ordering constraint worth knowing is that an SDP offer has to name the port audio will
//! arrive on, and only a bound socket knows that port. So the media socket is bound *before*
//! the INVITE is sent, not after the answer comes back.

pub mod call;
pub mod dialog;
pub mod error;
pub mod event;
pub mod rel;
pub mod transfer;
pub mod update;

pub use call::{
    Call, DialOptions, answer, answer_early, answer_replacing, answer_ringing, dial, dial_once,
    serve,
};
pub use dialog::{Dialog, DialogId, Role};
pub use error::{Error, Result};
pub use event::{CallEvent, CallEvents, EndCause};
pub use rel::{Ringing, ring, ring_early};
pub use transfer::{Referral, Replaces, Transfer, TransferState};
