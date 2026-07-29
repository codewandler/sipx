//! The `sipx.app.v1` application contract: its types, its JSON wire format, and a sans-IO
//! instruction interpreter.
//!
//! # Experimental
//!
//! **This crate is experimental, and so is the wire line it speaks.**
//! [`docs/specs/app-contract.md`](../../../docs/specs/app-contract.md) says `sipx.app.v1` may
//! change incompatibly until two dissimilar applications have run against it — an inbound IVR and
//! an outbound notifier — after which a change requires a new line. Until then this crate's public
//! API and the bytes on the wire may both move in a patch release. Depend on it accordingly.
//!
//! # What is here
//!
//! - **The vocabulary.** [`Envelope`], [`CallSnapshot`] and [`EventKind`] are §5's events, host to
//!   app; [`Document`] and [`Instruction`] are §6's instructions, app to host. Serialization is
//!   confined to this crate: the JSON codec ([`json`]) and the base64 for `play.source.inline` are
//!   this crate's own, so no other crate in the workspace gains a dependency to speak the contract.
//! - **The interpreter.** [`Interpreter`] is §§5–6 as a state machine: [`Input`] in, [`Output`]
//!   out. It answers the question "what should the call framework do next", and nothing else.
//!
//! # Sans-IO
//!
//! §2: *the interpreter is sans-IO*. Nothing in this crate opens a socket, reads a clock, or
//! wants an async runtime. Time enters as [`Input::TimerFired`] and leaves as
//! [`Output::SetTimer`]; the app is reached by handing [`Output::Deliver`] to a driver and
//! bringing its answer back as [`Input::Response`]. The bindings §§7–8 describe — a webhook
//! server, a WebSocket peer, an embedded runtime — are drivers over this one machine.
//!
//! ```
//! use sipx_app_protocol::{
//!     CallSnapshot, Direction, EventKind, Input, Interpreter, Output, Policy, Timestamp,
//! };
//!
//! let call = CallSnapshot::new("call-1", Direction::Inbound);
//! let mut interpreter = Interpreter::new(call, Policy::default());
//! // The driver's clock reading is a parameter, never a field and never a `now()`.
//! let now = Timestamp::from_unix_millis(1_772_270_104_221);
//! let outputs = interpreter.handle(now, Input::Event(EventKind::Incoming));
//! // The call has no program yet, so the one thing to do is ask the app for one.
//! assert!(matches!(outputs.first(), Some(Output::Deliver { .. })));
//! ```

mod base64;
mod document;
mod error;
mod event;
mod interpreter;
mod policy;
mod program;
mod tagged;
mod time;

pub mod json;
pub mod testing;

#[cfg(feature = "call")]
mod call;

pub use crate::document::{Document, DtmfMode, Gather, Instruction, Source, TransferTarget, Verb};
pub use crate::error::{Error, Result};
pub use crate::event::{
    CONTRACT, CallSnapshot, CallState, DialOutcome, Direction, EndCause, Envelope, EventKind,
    GatherReason, Headers, Leg, MediaState, TransferState,
};
pub use crate::interpreter::{
    Callback, Effect, Input, Interpreter, MAX_QUEUED_EVENTS, Output, Response, Timer,
};
pub use crate::policy::{Failure, OnFailure, Policy};
pub use crate::time::Timestamp;

#[cfg(feature = "call")]
pub use crate::call::event_from_call;
