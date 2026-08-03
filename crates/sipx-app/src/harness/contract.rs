//! The `sipx.app.v1` vocabulary, as Rust types rather than as JSON.
//!
//! [`specs/app-contract.md`](../../../../docs/specs/app-contract.md) §5 and §6. There is no
//! serialization here. This vocabulary predates the `C-5` protocol crate and remains the public
//! `A-7` harness's provisional model. It is a second representation of the program rather than the
//! sole contract implementation; migrating the harness to `sipx_app_protocol::Interpreter` is an
//! open `A-2` requirement. Production document mode does not use these types.
//!
//! Only the subset §11's vectors exercise is modelled. A verb the host would execute but no vector
//! covers is [`Verb::Other`], which carries its name and is executed as a no-op effect; a verb the
//! contract does not define at all is [`Verb::Unknown`], which §6.4 says makes the whole document
//! an error.

use std::time::Duration;

/// Why a call ended (§5.3, `call.ended`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndCause {
    /// The app asked for it.
    Hangup,
    /// The far end did.
    Remote,
    /// Refused with a status.
    Rejected {
        /// The status it was refused with.
        status: u16,
    },
    /// Nobody answered in time.
    Timeout,
    /// The host could not go on.
    Error,
}

/// How a `gather` resolved (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatherReason {
    /// A terminator key was pressed.
    Terminator,
    /// `max` digits arrived.
    Max,
    /// Nothing arrived in time.
    Timeout,
}

/// How a `dial` resolved (§5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialOutcome {
    /// The far end picked up.
    Answered,
    /// 486.
    Busy,
    /// Any other refusal.
    Rejected {
        /// The status the far end gave.
        status: u16,
    },
    /// It never resolved.
    Timeout,
}

/// What happened to a call (§5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// A new INVITE matched this app.
    Incoming,
    /// A provisional was sent or received.
    Ringing {
        /// Whether it was reliable (RFC 3262).
        reliable: bool,
    },
    /// The 2xx/ACK completed.
    Answered,
    /// One full keypress.
    Dtmf {
        /// Which key.
        digit: char,
        /// How long it was held.
        duration_ms: u64,
    },
    /// A `play` ran out or was cut.
    PlaybackFinished {
        /// The instruction it completes.
        instruction_id: String,
        /// Whether it ran to the end.
        completed: bool,
    },
    /// A `gather` resolved.
    GatherFinished {
        /// The instruction it completes.
        instruction_id: String,
        /// What was collected.
        digits: String,
        /// Why it stopped.
        reason: GatherReason,
    },
    /// A `dial` resolved.
    DialFinished {
        /// The instruction it completes.
        instruction_id: String,
        /// Which leg.
        leg: String,
        /// How it went.
        outcome: DialOutcome,
    },
    /// The call is over. Always last, and never dropped (AC-9).
    Ended {
        /// Why.
        cause: EndCause,
    },
}

impl EventKind {
    /// Whether this is the call's last word.
    ///
    /// The one question the event queue's overflow policy asks, which is why it is a method here
    /// rather than a `matches!` repeated at each place that drops.
    #[must_use]
    pub fn is_ended(&self) -> bool {
        matches!(self, Self::Ended { .. })
    }

    /// The type name as §5.3 spells it.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Incoming => "call.incoming",
            Self::Ringing { .. } => "call.ringing",
            Self::Answered => "call.answered",
            Self::Dtmf { .. } => "call.dtmf",
            Self::PlaybackFinished { .. } => "call.playback.finished",
            Self::GatherFinished { .. } => "call.gather.finished",
            Self::DialFinished { .. } => "call.dial.finished",
            Self::Ended { .. } => "call.ended",
        }
    }
}

/// An event as the app receives it: the §5.1 envelope, minus the parts only the wire needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Per-call, starts at 1, increments by 1. A redelivery repeats it (§5.1), which is what
    /// makes AC-4 expressible at all.
    pub seq: u64,
    /// What happened.
    pub kind: EventKind,
}

/// What an instruction asks the host to do (§6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    /// Answer the call.
    Answer,
    /// Play a clip. Blocks the queue until `call.playback.finished`.
    Play {
        /// A host-local source name (§6.5 — no URLs).
        source: String,
        /// Whether a digit stops it.
        interruptible: bool,
    },
    /// Collect digits. Blocks until `call.gather.finished`.
    Gather {
        /// How many digits end it.
        max: usize,
        /// Keys that end it early.
        terminators: String,
        /// How long to wait for the whole thing.
        timeout: Duration,
    },
    /// Place a leg. Blocks until `call.dial.finished`.
    Dial {
        /// Where to.
        target: String,
        /// How long to wait.
        timeout: Duration,
    },
    /// End the call.
    Hangup {
        /// Why.
        cause: EndCause,
    },
    /// Refuse the call with a status.
    Reject {
        /// The status.
        status: u16,
    },
    /// A contract verb this harness does not model. Executed as a no-op so a scenario can carry
    /// one without the document being an error.
    Other(String),
    /// A verb the contract does not define. §6.4: the document is rejected whole (AC-5).
    Unknown(String),
}

impl Verb {
    /// Whether this verb blocks the instruction queue until a completion event resolves it (§6.1).
    #[must_use]
    pub fn blocks(&self) -> bool {
        matches!(
            self,
            Self::Play { .. } | Self::Gather { .. } | Self::Dial { .. }
        )
    }
}

/// One instruction: a client-assigned id and a verb (§6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    /// Unique within the call, echoed as `instruction_id` on completion events. Correlation is
    /// the app's, not positional.
    pub id: String,
    /// What to do.
    pub verb: Verb,
}

impl Instruction {
    /// An instruction with this id and verb.
    pub fn new(id: impl Into<String>, verb: Verb) -> Self {
        Self {
            id: id.into(),
            verb,
        }
    }
}

/// A response document: the *entire* new program (§6.3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    /// In order. An empty list is valid and means "keep going".
    pub instructions: Vec<Instruction>,
}

impl Document {
    /// A document of these instructions.
    #[must_use]
    pub fn of(instructions: Vec<Instruction>) -> Self {
        Self { instructions }
    }

    /// The "keep going" document.
    #[must_use]
    pub fn keep_going() -> Self {
        Self::default()
    }

    /// Whether §6.4 rejects this document whole.
    ///
    /// One unknown verb condemns the document rather than being skipped: a host that ran the rest
    /// would run a different program than the app wrote, which §4 names as the reason.
    #[must_use]
    pub fn is_rejected(&self) -> Option<&str> {
        self.instructions.iter().find_map(|i| match &i.verb {
            Verb::Unknown(name) => Some(name.as_str()),
            _ => None,
        })
    }
}

/// What the host actually did — the observable output of a scenario.
///
/// These are what would be executed against `sipx-call`; the harness records them instead, which
/// is what makes the decision logic testable with no call in existence.
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
    /// Stop a playback that was running, because a new program replaced it (§6.3).
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
    /// A modelled-as-no-op contract verb ran.
    Other {
        /// The instruction.
        id: String,
        /// Its verb's name.
        verb: String,
    },
}
