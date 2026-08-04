//! Contract values used by the deterministic host driver.
//!
//! The vocabulary is re-exported from `sipx-app-protocol`; the harness deliberately has no
//! instruction or document model of its own. That keeps the fake-time path and the production
//! document-mode path on the same [`sipx_app_protocol::Interpreter`] semantics.

pub use sipx_app_protocol::{
    DialOutcome, Document, EndCause, EventKind, Gather, GatherReason, Instruction, Source, Verb,
};

/// An event as the scripted binding sees it.
///
/// The full snapshot remains owned by the protocol interpreter. The harness binding needs only
/// the two fields its scripts select on: delivery identity and event kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Per-call delivery sequence (§5.1).
    pub seq: u64,
    /// What happened.
    pub kind: EventKind,
}

impl Event {
    /// Whether this is the call's last event.
    #[must_use]
    pub fn is_ended(&self) -> bool {
        matches!(self.kind, EventKind::Ended { .. })
    }
}

/// What the scripted call driver performed.
///
/// This is an observation format, not an instruction vocabulary. Values are produced only by
/// matching [`sipx_app_protocol::Output`] from the sole interpreter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Answer the call.
    Answer,
    /// Start playing.
    StartPlay {
        /// The instruction.
        id: String,
        /// What to play.
        source: String,
    },
    /// Stop a playback superseded by a replacement program.
    StopPlay {
        /// The instruction that was running.
        id: String,
    },
    /// Begin collecting digits.
    StartGather {
        /// The instruction.
        id: String,
    },
    /// Place a leg.
    Dial {
        /// The instruction.
        id: String,
        /// Where to.
        target: String,
    },
    /// End the call.
    Hangup {
        /// Why.
        cause: EndCause,
    },
    /// Refuse it.
    Reject {
        /// The status.
        status: u16,
    },
    /// A contract effect outside the vector's observed subset ran.
    Other {
        /// Its operation name.
        verb: String,
    },
}
