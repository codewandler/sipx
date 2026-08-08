//! Events, host → app: the envelope, the call snapshot, and the event types.
//!
//! [`docs/specs/app-contract.md`](../../../../docs/specs/app-contract.md) §5. Two of the section's
//! claims are structural rather than documentary, and are made structural here:
//!
//! - **Events are authoritative.** Every envelope carries a full [`CallSnapshot`], never a delta,
//!   so there is no `Envelope` that leaves an app to infer state it did not receive.
//! - **`headers` is a selected set.** §5.2 says the snapshot never carries fields the host routes
//!   on. [`Headers`] enforces that by refusing anything outside the allowlist, so a host cannot
//!   leak `Via` or `Route` into a snapshot by accident — it would have to change this type.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::json::Json;
use crate::tagged;
use crate::time::Timestamp;

/// The wire line every envelope and document carries (§4).
pub const CONTRACT: &str = "sipx.app.v1";

/// Which way the call was set up (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Somebody called us.
    Inbound,
    /// We called somebody.
    Outbound,
}

impl Direction {
    /// Its spelling on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbound => "inbound",
            Self::Outbound => "outbound",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "inbound" => Some(Self::Inbound),
            "outbound" => Some(Self::Outbound),
            _ => None,
        }
    }
}

/// Where a call (or a leg) is (§5.2: `incoming · ringing · answered · ended`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    /// An INVITE arrived and nothing has been said back yet.
    Incoming,
    /// A provisional has been sent or received.
    Ringing,
    /// The 2xx/ACK completed.
    Answered,
    /// It is over.
    Ended,
}

impl CallState {
    /// Its spelling on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Incoming => "incoming",
            Self::Ringing => "ringing",
            Self::Answered => "answered",
            Self::Ended => "ended",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "incoming" => Some(Self::Incoming),
            "ringing" => Some(Self::Ringing),
            "answered" => Some(Self::Answered),
            "ended" => Some(Self::Ended),
            _ => None,
        }
    }
}

/// The selected inbound header fields a snapshot may carry (§5.2).
///
/// Lowercased keys, decoded values, and **only these four**: `From`, `To`, `P-Asserted-Identity`,
/// `Diversion`. The list is a closed allowlist rather than a documented convention because the
/// same paragraph says the snapshot never carries what the host routes on — and a `Via` that
/// reached an app once is a `Via` an app will parse forever.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers(BTreeMap<String, String>);

impl Headers {
    /// The four field names §5.2 selects, lowercased.
    pub const SELECTED: [&'static str; 4] = ["from", "to", "p-asserted-identity", "diversion"];

    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a field name is one §5.2 selects. Case-insensitive, as SIP field names are.
    #[must_use]
    pub fn is_selected(name: &str) -> bool {
        let lowered = name.to_ascii_lowercase();
        Self::SELECTED.contains(&lowered.as_str())
    }

    /// Record a field, if it is one of the selected ones.
    ///
    /// Returns whether it was taken. A caller that ignores the answer has not leaked anything —
    /// the value is simply not there.
    pub fn set(&mut self, name: &str, value: impl Into<String>) -> bool {
        if !Self::is_selected(name) {
            return false;
        }
        self.0.insert(name.to_ascii_lowercase(), value.into());
        true
    }

    /// One field, by its lowercased name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    /// Every field, in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn to_json(&self) -> Json {
        Json::Object(
            self.0
                .iter()
                .map(|(k, v)| (k.clone(), Json::Str(v.clone())))
                .collect(),
        )
    }

    fn from_json(value: &Json) -> Self {
        let mut headers = Self::new();
        if let Some(members) = value.as_object() {
            for (name, field) in members {
                if let Some(text) = field.as_str() {
                    headers.set(name, text);
                }
            }
        }
        headers
    }
}

/// What the media is doing (§5.2's `media` member).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MediaState {
    /// Whether the RTP is keyed (SRTP).
    pub encrypted: bool,
    /// Whether the far end has this call on hold.
    pub on_hold: bool,
    /// Whether this side's own microphone is gated (`M-18`). Not hold: nothing was signalled.
    pub muted: bool,
}

impl MediaState {
    fn to_json(self) -> Json {
        Json::object([
            ("encrypted", Some(Json::from(self.encrypted))),
            ("on_hold", Some(Json::from(self.on_hold))),
            ("muted", Some(Json::from(self.muted))),
        ])
    }

    fn from_json(value: &Json) -> Self {
        Self {
            encrypted: value
                .get("encrypted")
                .and_then(Json::as_bool)
                .unwrap_or(false),
            on_hold: value
                .get("on_hold")
                .and_then(Json::as_bool)
                .unwrap_or(false),
            muted: value.get("muted").and_then(Json::as_bool).unwrap_or(false),
        }
    }
}

/// Another leg of the same call (§5.2's `legs` member) — what a `dial` created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leg {
    /// The leg's name, as the `dial` completion event reports it.
    pub leg: String,
    /// Where it is.
    pub state: CallState,
    /// Who it is to.
    pub to: String,
}

impl Leg {
    fn to_json(&self) -> Json {
        Json::object([
            ("leg", Some(Json::Str(self.leg.clone()))),
            ("state", Some(Json::Str(self.state.as_str().to_owned()))),
            ("to", Some(Json::Str(self.to.clone()))),
        ])
    }

    fn from_json(value: &Json) -> Result<Self> {
        Ok(Self {
            leg: string_field(value, "leg")?,
            state: value
                .get("state")
                .and_then(Json::as_str)
                .and_then(CallState::parse)
                .ok_or(Error::BadField { field: "state" })?,
            to: string_field(value, "to")?,
        })
    }
}

/// The full call snapshot every event carries (§5.2).
///
/// §2: *events are authoritative* — an app is expected to overwrite whatever it believes from
/// this on every event, so that a missed delivery cannot leave it permanently wrong. That is why
/// there is no delta type anywhere in this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallSnapshot {
    /// The call's identity, stable for its lifetime.
    pub id: String,
    /// Which leg of it this event is about.
    pub leg: String,
    /// Which way it was set up.
    pub direction: Direction,
    /// Where it is now — *now*, not where it was when a queued event happened (§6.3).
    pub state: CallState,
    /// Who it is from.
    pub from: String,
    /// Who it is to.
    pub to: String,
    /// The selected inbound header fields.
    pub headers: Headers,
    /// What the media is doing.
    pub media: MediaState,
    /// Other legs of the same call.
    pub legs: Vec<Leg>,
    /// Whether this leg is bridged to another.
    pub bridged: bool,
    /// Whatever the `tag` verb has recorded, which lands in every later snapshot (§6.2).
    pub tags: BTreeMap<String, String>,
}

impl CallSnapshot {
    /// A snapshot of a call that has just arrived.
    #[must_use]
    pub fn new(id: impl Into<String>, direction: Direction) -> Self {
        Self {
            id: id.into(),
            leg: "a".to_owned(),
            direction,
            state: match direction {
                Direction::Inbound => CallState::Incoming,
                Direction::Outbound => CallState::Ringing,
            },
            from: String::new(),
            to: String::new(),
            headers: Headers::new(),
            media: MediaState::default(),
            legs: Vec::new(),
            bridged: false,
            tags: BTreeMap::new(),
        }
    }

    /// With these endpoints.
    #[must_use]
    pub fn between(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.from = from.into();
        self.to = to.into();
        self
    }

    /// This snapshot as JSON (§5.2).
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object([
            ("id", Some(Json::Str(self.id.clone()))),
            ("leg", Some(Json::Str(self.leg.clone()))),
            (
                "direction",
                Some(Json::Str(self.direction.as_str().to_owned())),
            ),
            ("state", Some(Json::Str(self.state.as_str().to_owned()))),
            ("from", Some(Json::Str(self.from.clone()))),
            ("to", Some(Json::Str(self.to.clone()))),
            ("headers", Some(self.headers.to_json())),
            ("media", Some(self.media.to_json())),
            (
                "legs",
                Some(Json::Array(self.legs.iter().map(Leg::to_json).collect())),
            ),
            ("bridged", Some(Json::from(self.bridged))),
            (
                "tags",
                Some(Json::Object(
                    self.tags
                        .iter()
                        .map(|(k, v)| (k.clone(), Json::Str(v.clone())))
                        .collect(),
                )),
            ),
        ])
    }

    /// Read a snapshot back.
    ///
    /// # Errors
    ///
    /// [`Error::MissingField`] or [`Error::BadField`] for a member the vocabulary requires and
    /// this document does not have a reading for.
    pub fn from_json(value: &Json) -> Result<Self> {
        Ok(Self {
            id: string_field(value, "id")?,
            leg: string_field(value, "leg")?,
            direction: value
                .get("direction")
                .and_then(Json::as_str)
                .and_then(Direction::parse)
                .ok_or(Error::BadField { field: "direction" })?,
            state: value
                .get("state")
                .and_then(Json::as_str)
                .and_then(CallState::parse)
                .ok_or(Error::BadField { field: "state" })?,
            from: string_field(value, "from")?,
            to: string_field(value, "to")?,
            headers: value
                .get("headers")
                .map(Headers::from_json)
                .unwrap_or_default(),
            media: value
                .get("media")
                .map(MediaState::from_json)
                .unwrap_or_default(),
            legs: match value.get("legs").and_then(Json::as_array) {
                Some(items) => items.iter().map(Leg::from_json).collect::<Result<_>>()?,
                None => Vec::new(),
            },
            bridged: value
                .get("bridged")
                .and_then(Json::as_bool)
                .unwrap_or(false),
            tags: value
                .get("tags")
                .and_then(Json::as_object)
                .map(|members| {
                    members
                        .iter()
                        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

/// Why a call ended (§5.3, `call.ended`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndCause {
    /// The app asked for it.
    Hangup,
    /// The far end did.
    Remote,
    /// This side refused the invitation with a status.
    Rejected {
        /// The status it was refused with.
        status: u16,
    },
    /// The far end stopped answering.
    Timeout,
    /// The host could not go on.
    Error,
}

impl EndCause {
    /// Its spelling on the wire, in §5.3's tagged form: a name with no fields is the bare string
    /// (`"busy"`), and a name with fields is an object carrying them (`{"name": "rejected",
    /// "status": 486}`).
    #[must_use]
    pub fn to_json(self) -> Json {
        match self {
            Self::Hangup => tagged::name("hangup"),
            Self::Remote => tagged::name("remote"),
            Self::Rejected { status } => tagged::with_status("rejected", status),
            Self::Timeout => tagged::name("timeout"),
            Self::Error => tagged::name("error"),
        }
    }

    pub(crate) fn from_json(value: &Json) -> Option<Self> {
        match tagged::read(value)? {
            ("hangup", _) => Some(Self::Hangup),
            ("remote", _) => Some(Self::Remote),
            ("rejected", status) => Some(Self::Rejected {
                status: status.unwrap_or(500),
            }),
            ("timeout", _) => Some(Self::Timeout),
            ("error", _) => Some(Self::Error),
            _ => None,
        }
    }
}

/// How a `gather` resolved (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatherReason {
    /// A terminator key was pressed.
    Terminator,
    /// `max` digits arrived.
    Max,
    /// Nothing more arrived in time.
    Timeout,
}

impl GatherReason {
    /// Its spelling on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terminator => "terminator",
            Self::Max => "max",
            Self::Timeout => "timeout",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "terminator" => Some(Self::Terminator),
            "max" => Some(Self::Max),
            "timeout" => Some(Self::Timeout),
            _ => None,
        }
    }
}

/// How a `dial` resolved (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl DialOutcome {
    /// Its spelling on the wire, in §5.3's tagged form: a name with no fields is the bare string
    /// (`"busy"`), and a name with fields is an object carrying them (`{"name": "rejected",
    /// "status": 486}`).
    #[must_use]
    pub fn to_json(self) -> Json {
        match self {
            Self::Answered => tagged::name("answered"),
            Self::Busy => tagged::name("busy"),
            Self::Rejected { status } => tagged::with_status("rejected", status),
            Self::Timeout => tagged::name("timeout"),
        }
    }

    pub(crate) fn from_json(value: &Json) -> Option<Self> {
        match tagged::read(value)? {
            ("answered", _) => Some(Self::Answered),
            ("busy", _) => Some(Self::Busy),
            ("rejected", status) => Some(Self::Rejected {
                status: status.unwrap_or(603),
            }),
            ("timeout", _) => Some(Self::Timeout),
            _ => None,
        }
    }
}

/// Which side of a call's audio an observation belongs to (§5.3, `call.voice.*`).
///
/// Not [`Direction`], which says who called whom: an inbound call's *outbound* audio is this side
/// speaking, and folding the two would make "the caller stopped talking" unreadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioDirection {
    /// Audio received from the far end.
    Inbound,
    /// Audio this side sent.
    Outbound,
}

impl AudioDirection {
    /// Its spelling on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbound => "inbound",
            Self::Outbound => "outbound",
        }
    }

    /// Read one back, or `None` for a word this version does not define.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "inbound" => Some(Self::Inbound),
            "outbound" => Some(Self::Outbound),
            _ => None,
        }
    }
}

/// Why voice ended (§5.3, `call.voice.ended`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceEndCause {
    /// The configured hangover elapsed with no further voice.
    Hangover,
    /// A reset cut it: a discontinuity in the audio, a format change, or the call's audio
    /// finishing. An application that was told voice started is always told it ended.
    Cut,
}

impl VoiceEndCause {
    /// Its spelling on the wire, in §5.3's tagged form.
    #[must_use]
    pub fn to_json(self) -> Json {
        match self {
            Self::Hangover => tagged::name("hangover"),
            Self::Cut => tagged::name("cut"),
        }
    }

    pub(crate) fn from_json(value: &Json) -> Option<Self> {
        match tagged::read(value)? {
            ("hangover", _) => Some(Self::Hangover),
            ("cut", _) => Some(Self::Cut),
            _ => None,
        }
    }
}

/// How far a transfer this side asked for has got (§5.3, `call.transfer.progress`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
    /// The REFER was accepted and nothing has happened yet.
    Trying,
    /// The transfer target is ringing.
    Ringing,
    /// It connected.
    Succeeded,
    /// It did not.
    Failed {
        /// The status the transfer target or the transferee gave.
        status: u16,
    },
}

impl TransferState {
    /// Its spelling on the wire, in §5.3's tagged form: a name with no fields is the bare string
    /// (`"busy"`), and a name with fields is an object carrying them (`{"name": "rejected",
    /// "status": 486}`).
    #[must_use]
    pub fn to_json(self) -> Json {
        match self {
            Self::Trying => tagged::name("trying"),
            Self::Ringing => tagged::name("ringing"),
            Self::Succeeded => tagged::name("succeeded"),
            Self::Failed { status } => tagged::with_status("failed", status),
        }
    }

    pub(crate) fn from_json(value: &Json) -> Option<Self> {
        match tagged::read(value)? {
            ("trying", _) => Some(Self::Trying),
            ("ringing", _) => Some(Self::Ringing),
            ("succeeded", _) => Some(Self::Succeeded),
            ("failed", status) => Some(Self::Failed {
                status: status.unwrap_or(500),
            }),
            _ => None,
        }
    }
}

/// What happened, as §5.3's table of event types.
///
/// The `instruction_id` members are what §6.1's client-assigned ids are *for*: correlation is the
/// app's, not positional, so a completion event names the instruction it completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// A new INVITE reached the host and matched this app.
    Incoming,
    /// A provisional was sent or received.
    Ringing {
        /// Whether it was reliable (RFC 3262).
        reliable: bool,
    },
    /// A reliable provisional completed offer/answer and its early media is running.
    EarlyMediaStarted,
    /// The 2xx/ACK completed; media may flow.
    Answered,
    /// An RFC 4733 event ended: one full keypress.
    Dtmf {
        /// Which key.
        digit: char,
        /// How long it was held.
        duration_ms: u32,
    },
    /// Deterministic signal analysis found voice on one side of the call's audio.
    ///
    /// Never recognition: no speech model is loaded to produce this, and a host with no speech
    /// runtime still emits it.
    VoiceStarted {
        /// Which side of the audio.
        direction: AudioDirection,
        /// This call's voice-observation number, from 0.
        sequence: u64,
        /// Where it sat, in samples from the start of the current analysis epoch.
        sample_time: u64,
        /// The rate `sample_time` is counted at.
        sample_rate: u32,
    },
    /// That voice stopped, at the end of the last active window.
    VoiceEnded {
        /// Which side of the audio.
        direction: AudioDirection,
        /// This call's voice-observation number, from 0.
        sequence: u64,
        /// Where it sat, in samples from the start of the current analysis epoch.
        sample_time: u64,
        /// The rate `sample_time` is counted at.
        sample_rate: u32,
        /// Whether the hangover elapsed or a reset cut it.
        cause: VoiceEndCause,
    },
    /// A reporting period of the call's audio completed (`M-59`).
    ///
    /// Signal *content* — what the audio carried — and never packet delivery: loss, jitter,
    /// round-trip time and the MOS estimate are the media stack's RTP/RTCP surface and appear
    /// nowhere here.
    SignalMetrics {
        /// Which side of the audio.
        direction: AudioDirection,
        /// The measurement run, advanced whenever the analysis timeline restarts.
        epoch: u64,
        /// This report's number within its epoch, from 0.
        sequence: u64,
        /// The first sample covered, in samples from the start of the epoch.
        sample_time: u64,
        /// The rate `sample_time` and `samples` are counted at.
        sample_rate: u32,
        /// How many samples the report covers.
        samples: u64,
        /// How many analysis windows the report covers.
        windows: u32,
        /// The largest sample magnitude over the coverage, 0..=32,768.
        peak: u32,
        /// `floor(sqrt(floor(Σs² / samples)))`, in the same amplitude units as `peak`.
        rms: u32,
        /// How many samples sat at full scale, either sign.
        clipped_samples: u64,
        /// How many covered windows clipped.
        clipping_windows: u32,
        /// How many covered windows met the activation threshold on DC-free variance.
        active_windows: u32,
        /// How many covered windows stayed below the silence floor.
        silent_windows: u32,
    },
    /// Unbroken silence in the call's audio reached the configured timeout (`M-59`).
    SignalSilence {
        /// Which side of the audio.
        direction: AudioDirection,
        /// The measurement run the silent run belongs to.
        epoch: u64,
        /// The first sample of the silent run, from the start of the epoch.
        sample_time: u64,
        /// The rate `sample_time` is counted at.
        sample_rate: u32,
    },
    /// A `play` ran out or was cut.
    PlaybackFinished {
        /// The `play` this completes.
        instruction_id: String,
        /// Whether it ran to the end.
        completed: bool,
    },
    /// A `gather` resolved.
    GatherFinished {
        /// The `gather` this completes.
        instruction_id: String,
        /// What was collected. Empty on a timeout with nothing pressed.
        digits: String,
        /// Why it stopped.
        reason: GatherReason,
    },
    /// A `record` resolved.
    RecordingFinished {
        /// The `record` this completes.
        instruction_id: String,
        /// How much audio there is.
        duration_ms: u32,
    },
    /// A `dial` resolved.
    DialFinished {
        /// The `dial` this completes.
        instruction_id: String,
        /// The leg it created.
        leg: String,
        /// How it went.
        outcome: DialOutcome,
    },
    /// An inbound REFER arrived; the app must decide (§6.3).
    TransferRequested {
        /// Where the transferor wants the call sent.
        target: String,
        /// Whether the `Refer-To` carries a `Replaces`.
        attended: bool,
    },
    /// A NOTIFY moved a transfer this side asked for.
    TransferProgress {
        /// How far it has got.
        state: TransferState,
    },
    /// The media coupling was made.
    Bridged {
        /// The other leg.
        leg: String,
    },
    /// The media coupling was broken.
    Unbridged {
        /// The other leg.
        leg: String,
    },
    /// The far end changed the media direction to hold.
    Hold,
    /// The far end took it off hold.
    Resumed,
    /// The call is over. Always the last event, never dropped.
    Ended {
        /// Why.
        cause: EndCause,
    },
    /// An event type this version of the vocabulary does not define.
    ///
    /// §4: an unknown *event type* must be ignored by the app — which is a thing an app can only
    /// do if reading one is not an error. So this is a variant rather than an [`Error`], and it is
    /// the one variant the interpreter never produces.
    Other {
        /// The type as it was spelled.
        type_name: String,
    },
}

impl EventKind {
    /// Its `type` on the wire (§5.3's first column).
    #[must_use]
    pub fn type_name(&self) -> &str {
        match self {
            Self::Incoming => "call.incoming",
            Self::Ringing { .. } => "call.ringing",
            Self::EarlyMediaStarted => "call.early_media.started",
            Self::Answered => "call.answered",
            Self::Dtmf { .. } => "call.dtmf",
            Self::VoiceStarted { .. } => "call.voice.started",
            Self::VoiceEnded { .. } => "call.voice.ended",
            Self::SignalMetrics { .. } => "call.signal.metrics",
            Self::SignalSilence { .. } => "call.signal.silence",
            Self::PlaybackFinished { .. } => "call.playback.finished",
            Self::GatherFinished { .. } => "call.gather.finished",
            Self::RecordingFinished { .. } => "call.recording.finished",
            Self::DialFinished { .. } => "call.dial.finished",
            Self::TransferRequested { .. } => "call.transfer.requested",
            Self::TransferProgress { .. } => "call.transfer.progress",
            Self::Bridged { .. } => "call.bridged",
            Self::Unbridged { .. } => "call.unbridged",
            Self::Hold => "call.hold",
            Self::Resumed => "call.resumed",
            Self::Ended { .. } => "call.ended",
            Self::Other { type_name } => type_name,
        }
    }

    /// Every event type §5.3 defines, in the section's order.
    ///
    /// Enumerable so that "the crate covers the table" is a test rather than a promise; the
    /// derived test in `tests/spec_tables.rs` reads the section and compares.
    #[must_use]
    pub fn type_names() -> [&'static str; 20] {
        [
            "call.incoming",
            "call.ringing",
            "call.early_media.started",
            "call.answered",
            "call.dtmf",
            "call.voice.started",
            "call.voice.ended",
            "call.signal.metrics",
            "call.signal.silence",
            "call.playback.finished",
            "call.gather.finished",
            "call.recording.finished",
            "call.dial.finished",
            "call.transfer.requested",
            "call.transfer.progress",
            "call.bridged",
            "call.unbridged",
            "call.hold",
            "call.resumed",
            "call.ended",
        ]
    }

    /// The `instruction_id` this event correlates against, if it is a completion event.
    #[must_use]
    pub fn instruction_id(&self) -> Option<&str> {
        match self {
            Self::PlaybackFinished { instruction_id, .. }
            | Self::GatherFinished { instruction_id, .. }
            | Self::RecordingFinished { instruction_id, .. }
            | Self::DialFinished { instruction_id, .. } => Some(instruction_id),
            _ => None,
        }
    }

    /// This event as the envelope's `event` member: its `type`, plus §5.3's extra fields.
    #[must_use]
    pub fn to_json(&self) -> Json {
        let mut members: Vec<(&'static str, Option<Json>)> =
            vec![("type", Some(Json::Str(self.type_name().to_owned())))];
        match self {
            Self::Incoming
            | Self::EarlyMediaStarted
            | Self::Answered
            | Self::Hold
            | Self::Resumed
            | Self::Other { .. } => {}
            Self::Ringing { reliable } => members.push(("reliable", Some(Json::from(*reliable)))),
            Self::Dtmf { digit, duration_ms } => {
                members.push(("digit", Some(Json::Str(digit.to_string()))));
                members.push(("duration_ms", Some(Json::from(*duration_ms))));
            }
            Self::VoiceStarted {
                direction,
                sequence,
                sample_time,
                sample_rate,
            } => {
                members.push(("direction", Some(Json::Str(direction.as_str().to_owned()))));
                members.push(("sequence", Some(Json::from(*sequence))));
                members.push(("sample_time", Some(Json::from(*sample_time))));
                members.push(("sample_rate", Some(Json::from(*sample_rate))));
            }
            Self::VoiceEnded {
                direction,
                sequence,
                sample_time,
                sample_rate,
                cause,
            } => {
                members.push(("direction", Some(Json::Str(direction.as_str().to_owned()))));
                members.push(("sequence", Some(Json::from(*sequence))));
                members.push(("sample_time", Some(Json::from(*sample_time))));
                members.push(("sample_rate", Some(Json::from(*sample_rate))));
                members.push(("cause", Some(cause.to_json())));
            }
            Self::SignalMetrics { .. } | Self::SignalSilence { .. } => {
                members.extend(self.signal_members());
            }
            Self::PlaybackFinished {
                instruction_id,
                completed,
            } => {
                members.push(("instruction_id", Some(Json::Str(instruction_id.clone()))));
                members.push(("completed", Some(Json::from(*completed))));
            }
            Self::GatherFinished {
                instruction_id,
                digits,
                reason,
            } => {
                members.push(("instruction_id", Some(Json::Str(instruction_id.clone()))));
                members.push(("digits", Some(Json::Str(digits.clone()))));
                members.push(("reason", Some(Json::Str(reason.as_str().to_owned()))));
            }
            Self::RecordingFinished {
                instruction_id,
                duration_ms,
            } => {
                members.push(("instruction_id", Some(Json::Str(instruction_id.clone()))));
                members.push(("duration_ms", Some(Json::from(*duration_ms))));
            }
            Self::DialFinished {
                instruction_id,
                leg,
                outcome,
            } => {
                members.push(("instruction_id", Some(Json::Str(instruction_id.clone()))));
                members.push(("leg", Some(Json::Str(leg.clone()))));
                members.push(("outcome", Some(outcome.to_json())));
            }
            Self::TransferRequested { target, attended } => {
                members.push(("target", Some(Json::Str(target.clone()))));
                members.push(("attended", Some(Json::from(*attended))));
            }
            Self::TransferProgress { state } => members.push(("state", Some(state.to_json()))),
            Self::Bridged { leg } | Self::Unbridged { leg } => {
                members.push(("leg", Some(Json::Str(leg.clone()))));
            }
            Self::Ended { cause } => members.push(("cause", Some(cause.to_json()))),
        }
        Json::object(members)
    }

    /// Read an event back.
    ///
    /// An unrecognised `type` becomes [`Self::Other`] rather than an error, because §4 requires an
    /// app to be able to ignore one.
    ///
    /// # Errors
    ///
    /// [`Error::MissingField`] or [`Error::BadField`] when a known type is missing an extra field
    /// its row names, or carries one with no valid reading.
    pub fn from_json(value: &Json) -> Result<Self> {
        let type_name = value
            .get("type")
            .and_then(Json::as_str)
            .ok_or(Error::MissingField { field: "type" })?;
        Ok(match type_name {
            "call.incoming" => Self::Incoming,
            "call.ringing" => Self::Ringing {
                reliable: bool_field(value, "reliable")?,
            },
            "call.early_media.started" => Self::EarlyMediaStarted,
            "call.answered" => Self::Answered,
            "call.dtmf" => Self::Dtmf {
                digit: string_field(value, "digit")?
                    .chars()
                    .next()
                    .ok_or(Error::BadField { field: "digit" })?,
                duration_ms: u32_field(value, "duration_ms")?,
            },
            "call.voice.started" => Self::VoiceStarted {
                direction: audio_direction_field(value)?,
                sequence: u64_field(value, "sequence")?,
                sample_time: u64_field(value, "sample_time")?,
                sample_rate: u32_field(value, "sample_rate")?,
            },
            "call.voice.ended" => Self::VoiceEnded {
                direction: audio_direction_field(value)?,
                sequence: u64_field(value, "sequence")?,
                sample_time: u64_field(value, "sample_time")?,
                sample_rate: u32_field(value, "sample_rate")?,
                cause: value
                    .get("cause")
                    .and_then(VoiceEndCause::from_json)
                    .ok_or(Error::BadField { field: "cause" })?,
            },
            "call.signal.metrics" | "call.signal.silence" => signal_event(type_name, value)?,
            "call.playback.finished" => Self::PlaybackFinished {
                instruction_id: string_field(value, "instruction_id")?,
                completed: bool_field(value, "completed")?,
            },
            "call.gather.finished" => Self::GatherFinished {
                instruction_id: string_field(value, "instruction_id")?,
                digits: string_field(value, "digits")?,
                reason: value
                    .get("reason")
                    .and_then(Json::as_str)
                    .and_then(GatherReason::parse)
                    .ok_or(Error::BadField { field: "reason" })?,
            },
            "call.recording.finished" => Self::RecordingFinished {
                instruction_id: string_field(value, "instruction_id")?,
                duration_ms: u32_field(value, "duration_ms")?,
            },
            "call.dial.finished" => Self::DialFinished {
                instruction_id: string_field(value, "instruction_id")?,
                leg: string_field(value, "leg")?,
                outcome: value
                    .get("outcome")
                    .and_then(DialOutcome::from_json)
                    .ok_or(Error::BadField { field: "outcome" })?,
            },
            "call.transfer.requested" => Self::TransferRequested {
                target: string_field(value, "target")?,
                attended: bool_field(value, "attended")?,
            },
            "call.transfer.progress" => Self::TransferProgress {
                state: value
                    .get("state")
                    .and_then(TransferState::from_json)
                    .ok_or(Error::BadField { field: "state" })?,
            },
            "call.bridged" => Self::Bridged {
                leg: string_field(value, "leg")?,
            },
            "call.unbridged" => Self::Unbridged {
                leg: string_field(value, "leg")?,
            },
            "call.hold" => Self::Hold,
            "call.resumed" => Self::Resumed,
            "call.ended" => Self::Ended {
                cause: value
                    .get("cause")
                    .and_then(EndCause::from_json)
                    .ok_or(Error::BadField { field: "cause" })?,
            },
            other => Self::Other {
                type_name: other.to_owned(),
            },
        })
    }
}

/// One delivery, host → app (§5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    /// Per-call, starts at 1, increments by 1 per event. A repeated `seq` is a redelivery of the
    /// same event and must be treated as such (§5.1, vector AC-4).
    pub seq: u64,
    /// When it happened. Supplied by the driver — this crate reads no clock (§2).
    pub at: Timestamp,
    /// The full call snapshot, always current.
    pub call: CallSnapshot,
    /// What happened.
    pub event: EventKind,
}

impl Envelope {
    /// This envelope as JSON text: UTF-8, no BOM (§1, RFC 8259).
    #[must_use]
    pub fn to_text(&self) -> String {
        self.to_json().to_text()
    }

    /// This envelope as a JSON value.
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object([
            ("contract", Some(Json::Str(CONTRACT.to_owned()))),
            ("seq", Some(Json::from(self.seq))),
            ("at", Some(Json::Str(self.at.to_rfc3339()))),
            ("call", Some(self.call.to_json())),
            ("event", Some(self.event.to_json())),
        ])
    }

    /// Read an envelope off the wire — what an SDK on the app's side of the contract does.
    ///
    /// # Errors
    ///
    /// [`Error::Json`] for anything that is not JSON, [`Error::WrongContract`] for a different
    /// wire line, and [`Error::MissingField`] / [`Error::BadField`] for a malformed envelope.
    /// Never panics.
    pub fn from_text(text: &str) -> Result<Self> {
        let value = Json::parse(text)?;
        check_contract(&value)?;
        Ok(Self {
            seq: u64::try_from(
                value
                    .get("seq")
                    .and_then(Json::as_i64)
                    .ok_or(Error::MissingField { field: "seq" })?,
            )
            .map_err(|_| Error::BadField { field: "seq" })?,
            at: value
                .get("at")
                .and_then(Json::as_str)
                .and_then(Timestamp::from_rfc3339)
                .ok_or(Error::BadField { field: "at" })?,
            call: CallSnapshot::from_json(
                value
                    .get("call")
                    .ok_or(Error::MissingField { field: "call" })?,
            )?,
            event: EventKind::from_json(
                value
                    .get("event")
                    .ok_or(Error::MissingField { field: "event" })?,
            )?,
        })
    }
}

/// §4: the line is in every envelope and every document, and a different one is not ours to read.
pub(crate) fn check_contract(value: &Json) -> Result<()> {
    match value.get("contract").and_then(Json::as_str) {
        Some(CONTRACT) => Ok(()),
        Some(other) => Err(Error::WrongContract {
            found: Some(other.to_owned()),
        }),
        None => Err(Error::WrongContract { found: None }),
    }
}

pub(crate) fn string_field(value: &Json, field: &'static str) -> Result<String> {
    value
        .get(field)
        .and_then(Json::as_str)
        .map(str::to_owned)
        .ok_or(Error::MissingField { field })
}

pub(crate) fn bool_field(value: &Json, field: &'static str) -> Result<bool> {
    value
        .get(field)
        .and_then(Json::as_bool)
        .ok_or(Error::MissingField { field })
}

pub(crate) fn u32_field(value: &Json, field: &'static str) -> Result<u32> {
    let raw = value
        .get(field)
        .and_then(Json::as_i64)
        .ok_or(Error::MissingField { field })?;
    u32::try_from(raw).map_err(|_| Error::BadField { field })
}

pub(crate) fn u64_field(value: &Json, field: &'static str) -> Result<u64> {
    let raw = value
        .get(field)
        .and_then(Json::as_i64)
        .ok_or(Error::MissingField { field })?;
    u64::try_from(raw).map_err(|_| Error::BadField { field })
}

/// The `M-59` signal events' extra fields (§5.3).
///
/// Split out of [`EventKind::to_json`] because the two rows carry between them more fields than any
/// other event in the vocabulary, and a `match` arm that long makes the other fifteen harder to
/// read rather than these two easier.
impl EventKind {
    fn signal_members(&self) -> Vec<(&'static str, Option<Json>)> {
        match self {
            Self::SignalMetrics {
                direction,
                epoch,
                sequence,
                sample_time,
                sample_rate,
                samples,
                windows,
                peak,
                rms,
                clipped_samples,
                clipping_windows,
                active_windows,
                silent_windows,
            } => vec![
                ("direction", Some(Json::Str(direction.as_str().to_owned()))),
                ("epoch", Some(Json::from(*epoch))),
                ("sequence", Some(Json::from(*sequence))),
                ("sample_time", Some(Json::from(*sample_time))),
                ("sample_rate", Some(Json::from(*sample_rate))),
                ("samples", Some(Json::from(*samples))),
                ("windows", Some(Json::from(*windows))),
                ("peak", Some(Json::from(*peak))),
                ("rms", Some(Json::from(*rms))),
                ("clipped_samples", Some(Json::from(*clipped_samples))),
                ("clipping_windows", Some(Json::from(*clipping_windows))),
                ("active_windows", Some(Json::from(*active_windows))),
                ("silent_windows", Some(Json::from(*silent_windows))),
            ],
            Self::SignalSilence {
                direction,
                epoch,
                sample_time,
                sample_rate,
            } => vec![
                ("direction", Some(Json::Str(direction.as_str().to_owned()))),
                ("epoch", Some(Json::from(*epoch))),
                ("sample_time", Some(Json::from(*sample_time))),
                ("sample_rate", Some(Json::from(*sample_rate))),
            ],
            // Every other variant writes its own members; this is only reached through the two
            // arms above.
            _ => Vec::new(),
        }
    }
}

/// Read one of the two `M-59` signal events back (§5.3).
fn signal_event(type_name: &str, value: &Json) -> Result<EventKind> {
    Ok(match type_name {
        "call.signal.metrics" => EventKind::SignalMetrics {
            direction: audio_direction_field(value)?,
            epoch: u64_field(value, "epoch")?,
            sequence: u64_field(value, "sequence")?,
            sample_time: u64_field(value, "sample_time")?,
            sample_rate: u32_field(value, "sample_rate")?,
            samples: u64_field(value, "samples")?,
            windows: u32_field(value, "windows")?,
            peak: u32_field(value, "peak")?,
            rms: u32_field(value, "rms")?,
            clipped_samples: u64_field(value, "clipped_samples")?,
            clipping_windows: u32_field(value, "clipping_windows")?,
            active_windows: u32_field(value, "active_windows")?,
            silent_windows: u32_field(value, "silent_windows")?,
        },
        _ => EventKind::SignalSilence {
            direction: audio_direction_field(value)?,
            epoch: u64_field(value, "epoch")?,
            sample_time: u64_field(value, "sample_time")?,
            sample_rate: u32_field(value, "sample_rate")?,
        },
    })
}

fn audio_direction_field(value: &Json) -> Result<AudioDirection> {
    value
        .get("direction")
        .and_then(Json::as_str)
        .and_then(AudioDirection::parse)
        .ok_or(Error::BadField { field: "direction" })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn snapshot() -> CallSnapshot {
        let mut call = CallSnapshot::new("b7c1", Direction::Inbound)
            .between("sip:alice@example.com", "sip:support@example.net");
        call.state = CallState::Answered;
        call.headers
            .set("P-Asserted-Identity", "\"Alice\" <sip:alice@example.com>");
        call.media.encrypted = true;
        call.legs.push(Leg {
            leg: "b".to_owned(),
            state: CallState::Ringing,
            to: "sip:bob@example.net".to_owned(),
        });
        call.tags
            .insert("campaign".to_owned(), "renewal".to_owned());
        call
    }

    /// §5.1's own example, as a value: the members it shows, with the values it shows.
    #[test]
    fn the_spec_s_envelope_example_serializes_as_written() {
        let envelope = Envelope {
            seq: 4,
            at: Timestamp::from_rfc3339("2026-07-28T09:15:04.221Z").unwrap(),
            call: snapshot(),
            event: EventKind::Dtmf {
                digit: '5',
                duration_ms: 160,
            },
        };
        let value = Json::parse(&envelope.to_text()).unwrap();
        assert_eq!(value.get("contract").unwrap().as_str(), Some(CONTRACT));
        assert_eq!(value.get("seq").unwrap().as_i64(), Some(4));
        assert_eq!(
            value.get("at").unwrap().as_str(),
            Some("2026-07-28T09:15:04.221Z")
        );
        let event = value.get("event").unwrap();
        assert_eq!(event.get("type").unwrap().as_str(), Some("call.dtmf"));
        assert_eq!(event.get("digit").unwrap().as_str(), Some("5"));
        assert_eq!(event.get("duration_ms").unwrap().as_i64(), Some(160));
    }

    #[test]
    fn every_event_type_round_trips_through_the_wire() {
        let kinds = crate::testing::one_of_every_event();
        assert_eq!(kinds.len(), EventKind::type_names().len());
        for kind in kinds {
            let envelope = Envelope {
                seq: 1,
                at: Timestamp::from_unix_millis(0),
                call: snapshot(),
                event: kind.clone(),
            };
            let text = envelope.to_text();
            let back = Envelope::from_text(&text).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            assert_eq!(back, envelope, "{kind:?} did not survive {text}");
        }
    }

    /// §5.2: the snapshot never carries what the host routes on. Not a convention — the type
    /// refuses.
    #[test]
    fn a_routing_header_cannot_be_put_in_a_snapshot() {
        let mut headers = Headers::new();
        assert!(!headers.set("Via", "SIP/2.0/UDP host;branch=z9hG4bK1"));
        assert!(!headers.set("Route", "<sip:proxy.example.net;lr>"));
        assert!(!headers.set("CSeq", "1 INVITE"));
        assert!(headers.is_empty());
        assert!(headers.set("From", "<sip:alice@example.com>"));
        assert_eq!(headers.get("from"), Some("<sip:alice@example.com>"));
    }

    /// §5.3's two voice rows, field by field (`M-58`).
    ///
    /// The identity of the call is deliberately absent from the event body: it is the envelope's
    /// §5.2 snapshot `id`, and repeating it here would be a second spelling that can disagree.
    #[test]
    fn the_voice_events_carry_exactly_the_fields_section_5_3_names() {
        let started = EventKind::VoiceStarted {
            direction: AudioDirection::Outbound,
            sequence: 7,
            sample_time: 3_200,
            sample_rate: 16_000,
        }
        .to_json();
        assert_eq!(
            started.get("type").unwrap().as_str(),
            Some("call.voice.started")
        );
        assert_eq!(started.get("direction").unwrap().as_str(), Some("outbound"));
        assert_eq!(started.get("sequence").unwrap().as_i64(), Some(7));
        assert_eq!(started.get("sample_time").unwrap().as_i64(), Some(3_200));
        assert_eq!(started.get("sample_rate").unwrap().as_i64(), Some(16_000));
        assert!(
            started.get("call_id").is_none(),
            "the call is named by the envelope, once"
        );

        let ended = EventKind::VoiceEnded {
            direction: AudioDirection::Inbound,
            sequence: 8,
            sample_time: 3_360,
            sample_rate: 16_000,
            cause: VoiceEndCause::Cut,
        }
        .to_json();
        assert_eq!(
            ended.get("type").unwrap().as_str(),
            Some("call.voice.ended")
        );
        assert_eq!(ended.get("cause").unwrap().as_str(), Some("cut"));
    }

    /// §4: an unknown event type is the app's to ignore, so reading one must not be an error.
    #[test]
    fn an_unknown_event_type_reads_as_other_rather_than_failing() {
        let event = EventKind::from_json(&Json::parse(r#"{"type":"call.spindled"}"#).unwrap());
        assert_eq!(
            event,
            Ok(EventKind::Other {
                type_name: "call.spindled".to_owned()
            })
        );
    }

    #[test]
    fn a_different_wire_line_is_refused() {
        let envelope = Envelope {
            seq: 1,
            at: Timestamp::from_unix_millis(0),
            call: snapshot(),
            event: EventKind::Incoming,
        };
        let text = envelope.to_text().replace("sipx.app.v1", "sipx.app.v2");
        assert_eq!(
            Envelope::from_text(&text),
            Err(Error::WrongContract {
                found: Some("sipx.app.v2".to_owned())
            })
        );
    }

    #[test]
    fn nothing_a_peer_can_send_as_an_envelope_panics() {
        for text in crate::testing::HOSTILE_BODIES {
            let _ = Envelope::from_text(text);
        }
    }
}
