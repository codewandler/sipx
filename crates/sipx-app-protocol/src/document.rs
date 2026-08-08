//! Instructions, app → host: the document, the verbs, and their fields.
//!
//! [`docs/specs/app-contract.md`](../../../../docs/specs/app-contract.md) §6. The section's two
//! structural claims are made structural here rather than documented:
//!
//! - **§6.4: a document is rejected whole.** [`Document::parse`] returns one [`Error`] or one
//!   whole [`Document`]; there is no partially-read document for a caller to be tempted by, and no
//!   way to construct a [`Document`] holding instructions from a body that also had a bad one.
//! - **§6.5's two deliberate limits.** [`Source`] admits a host-local file or inline audio and has
//!   no URL variant at all, so "fetch by URL" is not a thing a `v1` document can say. `dial`'s
//!   header map is carried as written and filtered against the host's allowlist by the
//!   interpreter, which is the only place that knows what the host allows.

use std::collections::BTreeMap;

use crate::base64;
use crate::error::{Error, Result};
use crate::event::{CONTRACT, EndCause, check_contract, string_field};
use crate::json::Json;

/// Where a `play` gets its audio (§6.2, §6.5).
///
/// There is deliberately no URL variant. §6.5: fetching by URL is a host capability behind an
/// allowlist, outside this contract — and a variant here would make it representable, which is
/// exactly the property the limit exists to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A host-local file, named by the host's own resolution rules.
    File(String),
    /// PCM carried in the document itself, base64 (RFC 4648 §4) on the wire.
    Inline(Vec<u8>),
}

impl Source {
    fn to_json(&self) -> Json {
        match self {
            Self::File(path) => Json::object([("file", Some(Json::Str(path.clone())))]),
            Self::Inline(bytes) => {
                Json::object([("inline", Some(Json::Str(base64::encode(bytes))))])
            }
        }
    }

    fn from_json(value: &Json) -> Result<Self> {
        if let Some(path) = value.get("file").and_then(Json::as_str) {
            return Ok(Self::File(path.to_owned()));
        }
        if let Some(text) = value.get("inline").and_then(Json::as_str) {
            return base64::decode(text)
                .map(Self::Inline)
                .ok_or(Error::BadField { field: "inline" });
        }
        Err(Error::BadField { field: "source" })
    }
}

/// What a `bridge` does with digits from the far end (§6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DtmfMode {
    /// Let them through to the other leg.
    #[default]
    Passthrough,
    /// Keep them on this side, so the app still sees them and the other leg does not.
    Consume,
}

impl DtmfMode {
    /// Its spelling on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passthrough => "passthrough",
            Self::Consume => "consume",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "passthrough" => Some(Self::Passthrough),
            "consume" => Some(Self::Consume),
            _ => None,
        }
    }
}

/// Where a `transfer` sends the call (§6.2: `target` **or** `via_leg`).
///
/// An enum rather than two optional fields, so "neither" and "both" — the two readings §6.2 does
/// not define — are unrepresentable once a document has parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferTarget {
    /// A blind transfer to a URI (RFC 3515).
    Blind {
        /// Where to send it.
        target: String,
    },
    /// An attended transfer, putting the far end in the place of a leg this call already has
    /// (RFC 3891 `Replaces`).
    Attended {
        /// Which of this call's legs to replace.
        via_leg: String,
    },
}

/// What a `gather` collects and what stops it (§6.2).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Gather {
    /// Fewest digits that count as a result. A terminator pressed before this is reached does not
    /// resolve the gather.
    pub min: u32,
    /// Most digits to collect; reaching it resolves the gather with `reason: "max"`.
    pub max: Option<u32>,
    /// Keys that end collection, `reason: "terminator"`. The terminator is not part of `digits`.
    pub terminators: String,
    /// How long to wait for the *next* digit once at least one has arrived.
    pub digit_timeout_ms: Option<u32>,
    /// How long the whole gather may take.
    pub timeout_ms: Option<u32>,
    /// A prompt to play while collecting — interruptible by definition (§6.2), because a prompt
    /// that swallowed the digit that stopped it would lose the first digit of every entry.
    pub prompt: Option<Source>,
}

/// One instruction's verb and its fields (§6.2).
///
/// Every variant here has a row in §3's table, which is the rule that keeps the vocabulary honest:
/// the contract may not name a verb with no call-framework operation behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Verb {
    /// Accept the invitation. Completes with `call.answered`.
    Answer,
    /// Send a provisional.
    Ring {
        /// Whether to send it reliably (RFC 3262).
        reliable: bool,
    },
    /// Refuse the invitation. Completes with `call.ended`.
    Reject {
        /// The status to refuse with.
        status: u16,
        /// The reason phrase, if the app chose one.
        reason: Option<String>,
    },
    /// Play audio. Completes with `call.playback.finished`.
    Play {
        /// Where the audio comes from.
        source: Source,
        /// Whether a keypress cuts it short.
        interruptible: bool,
    },
    /// Collect digits. Completes with `call.gather.finished`.
    GatherDigits(Gather),
    /// Record the far end. Completes with `call.recording.finished`.
    Record {
        /// The longest recording to make.
        max_ms: Option<u32>,
        /// How much trailing silence ends it.
        idle_stop_ms: Option<u32>,
    },
    /// Send digits to the far end (RFC 4733). Immediate.
    SendDtmf {
        /// Which keys.
        digits: String,
        /// How long to hold each.
        duration_ms: Option<u32>,
    },
    /// Place a new leg of this call. Completes with `call.dial.finished`.
    Dial {
        /// Who to call.
        target: String,
        /// What to call as.
        from: Option<String>,
        /// How long to let it ring.
        timeout_ms: Option<u32>,
        /// Header fields to set, subject to the host's allowlist (§6.5).
        headers: BTreeMap<String, String>,
    },
    /// Couple this leg's media to another's. A *state*, ended by `unbridge` or a leg ending.
    Bridge {
        /// The other leg.
        leg: String,
        /// What to do with digits.
        dtmf: DtmfMode,
    },
    /// Break the media coupling.
    Unbridge,
    /// Put the far end on hold (a re-INVITE). Immediate; the outcome surfaces as events.
    Hold,
    /// Take it off hold.
    Resume,
    /// Gate this side's own audio (`M-18`). Not hold: nothing is signalled.
    Mute,
    /// Let it through again.
    Unmute,
    /// Transfer the call away (RFC 3515).
    Transfer {
        /// Where to.
        target: TransferTarget,
    },
    /// Accept a transfer the far end asked for.
    AcceptTransfer,
    /// Refuse one.
    RefuseTransfer {
        /// The status to refuse with.
        status: u16,
    },
    /// Do nothing for a while. Timer-driven, which is the one verb that makes the interpreter's
    /// timer inputs load-bearing rather than incidental.
    Pause {
        /// How long.
        ms: u32,
    },
    /// Record a key and value that lands in every later snapshot (§5.2's `tags`).
    Tag {
        /// The key.
        key: String,
        /// The value.
        value: String,
    },
    /// End the call. Completes with `call.ended`.
    Hangup {
        /// Why, for the snapshot and the event.
        cause: EndCause,
    },
}

impl Verb {
    /// Its spelling on the wire — §6.2's first column.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Answer => "answer",
            Self::Ring { .. } => "ring",
            Self::Reject { .. } => "reject",
            Self::Play { .. } => "play",
            Self::GatherDigits(_) => "gather",
            Self::Record { .. } => "record",
            Self::SendDtmf { .. } => "send_dtmf",
            Self::Dial { .. } => "dial",
            Self::Bridge { .. } => "bridge",
            Self::Unbridge => "unbridge",
            Self::Hold => "hold",
            Self::Resume => "resume",
            Self::Mute => "mute",
            Self::Unmute => "unmute",
            Self::Transfer { .. } => "transfer",
            Self::AcceptTransfer => "accept_transfer",
            Self::RefuseTransfer { .. } => "refuse_transfer",
            Self::Pause { .. } => "pause",
            Self::Tag { .. } => "tag",
            Self::Hangup { .. } => "hangup",
        }
    }

    /// Every verb §6.2 defines, in the section's order.
    ///
    /// Enumerable so that "the crate covers the table" is a test rather than a promise;
    /// `tests/spec_tables.rs` reads §6.2 out of the spec and compares against this.
    #[must_use]
    pub fn names() -> [&'static str; 20] {
        [
            "answer",
            "ring",
            "reject",
            "play",
            "gather",
            "record",
            "send_dtmf",
            "dial",
            "bridge",
            "unbridge",
            "hold",
            "resume",
            "mute",
            "unmute",
            "transfer",
            "accept_transfer",
            "refuse_transfer",
            "pause",
            "tag",
            "hangup",
        ]
    }

    /// Whether this verb blocks the queue until a completion event resolves it (§6.1).
    ///
    /// The other verbs complete the moment their effect is issued, so the queue moves straight on
    /// to the next one. This is the single place that distinction is written down.
    #[must_use]
    pub fn blocks(&self) -> bool {
        matches!(
            self,
            Self::Answer
                | Self::Play { .. }
                | Self::GatherDigits(_)
                | Self::Record { .. }
                | Self::Dial { .. }
                | Self::Pause { .. }
                | Self::Reject { .. }
                | Self::Hangup { .. }
        )
    }
}

/// One instruction: a client-assigned id and a verb (§6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instruction {
    /// §6.1: client-assigned, unique within the call, and echoed as `instruction_id` on the
    /// completion events of §5.3. Correlation is the app's, not positional.
    pub id: String,
    /// What to do.
    pub verb: Verb,
}

impl Instruction {
    /// A new instruction.
    pub fn new(id: impl Into<String>, verb: Verb) -> Self {
        Self {
            id: id.into(),
            verb,
        }
    }

    /// This instruction as JSON (§6.1).
    #[must_use]
    pub fn to_json(&self) -> Json {
        let mut members: Vec<(&'static str, Option<Json>)> = vec![
            ("id", Some(Json::Str(self.id.clone()))),
            ("do", Some(Json::Str(self.verb.name().to_owned()))),
        ];
        match &self.verb {
            Verb::Answer
            | Verb::Unbridge
            | Verb::Hold
            | Verb::Resume
            | Verb::Mute
            | Verb::Unmute
            | Verb::AcceptTransfer => {}
            Verb::Ring { reliable } => members.push(("reliable", Some(Json::from(*reliable)))),
            Verb::Reject { status, reason } => {
                members.push(("status", Some(Json::from(*status))));
                members.push(("reason", reason.clone().map(Json::Str)));
            }
            Verb::Play {
                source,
                interruptible,
            } => {
                members.push(("source", Some(source.to_json())));
                members.push(("interruptible", Some(Json::from(*interruptible))));
            }
            Verb::GatherDigits(gather) => {
                members.push(("min", Some(Json::from(gather.min))));
                members.push(("max", gather.max.map(Json::from)));
                members.push(("terminators", Some(Json::Str(gather.terminators.clone()))));
                members.push(("digit_timeout_ms", gather.digit_timeout_ms.map(Json::from)));
                members.push(("timeout_ms", gather.timeout_ms.map(Json::from)));
                members.push(("prompt", gather.prompt.as_ref().map(Source::to_json)));
            }
            Verb::Record {
                max_ms,
                idle_stop_ms,
            } => {
                members.push(("max_ms", max_ms.map(Json::from)));
                members.push(("idle_stop_ms", idle_stop_ms.map(Json::from)));
            }
            Verb::SendDtmf {
                digits,
                duration_ms,
            } => {
                members.push(("digits", Some(Json::Str(digits.clone()))));
                members.push(("duration_ms", duration_ms.map(Json::from)));
            }
            Verb::Dial {
                target,
                from,
                timeout_ms,
                headers,
            } => {
                members.push(("target", Some(Json::Str(target.clone()))));
                members.push(("from", from.clone().map(Json::Str)));
                members.push(("timeout_ms", timeout_ms.map(Json::from)));
                members.push((
                    "headers",
                    Some(Json::Object(
                        headers
                            .iter()
                            .map(|(k, v)| (k.clone(), Json::Str(v.clone())))
                            .collect(),
                    )),
                ));
            }
            Verb::Bridge { leg, dtmf } => {
                members.push(("leg", Some(Json::Str(leg.clone()))));
                members.push(("dtmf", Some(Json::Str(dtmf.as_str().to_owned()))));
            }
            Verb::Transfer { target } => match target {
                TransferTarget::Blind { target } => {
                    members.push(("target", Some(Json::Str(target.clone()))));
                }
                TransferTarget::Attended { via_leg } => {
                    members.push(("via_leg", Some(Json::Str(via_leg.clone()))));
                }
            },
            Verb::RefuseTransfer { status } => {
                members.push(("status", Some(Json::from(*status))));
            }
            Verb::Pause { ms } => members.push(("ms", Some(Json::from(*ms)))),
            Verb::Tag { key, value } => {
                members.push(("key", Some(Json::Str(key.clone()))));
                members.push(("value", Some(Json::Str(value.clone()))));
            }
            Verb::Hangup { cause } => members.push(("cause", Some(cause.to_json()))),
        }
        Json::object(members)
    }

    /// §6.2's verbs that act on the call itself.
    ///
    /// One arm per §6.2 row, in four functions rather than one. The grouping is the spec's own;
    /// the reason for splitting at all is that the whole table in a single `match` is longer than
    /// the workspace's function-length limit. `Ok(None)` means "not one of mine, try the next".
    fn control_verb(name: &str, value: &Json) -> Result<Option<Verb>> {
        Ok(Some(match name {
            "answer" => Verb::Answer,
            "ring" => Verb::Ring {
                reliable: value
                    .get("reliable")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
            },
            "reject" => Verb::Reject {
                status: status_field(value, "status")?,
                reason: value
                    .get("reason")
                    .and_then(Json::as_str)
                    .map(str::to_owned),
            },
            "hold" => Verb::Hold,
            "resume" => Verb::Resume,
            "mute" => Verb::Mute,
            "unmute" => Verb::Unmute,
            "hangup" => Verb::Hangup {
                cause: match value.get("cause") {
                    Some(cause) => {
                        EndCause::from_json(cause).ok_or(Error::BadField { field: "cause" })?
                    }
                    None => EndCause::Hangup,
                },
            },
            _ => return Ok(None),
        }))
    }

    /// §6.2's verbs that move audio.
    fn media_verb(name: &str, value: &Json) -> Result<Option<Verb>> {
        Ok(Some(match name {
            "play" => Verb::Play {
                source: Source::from_json(
                    value
                        .get("source")
                        .ok_or(Error::MissingField { field: "source" })?,
                )?,
                interruptible: value
                    .get("interruptible")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
            },
            "gather" => Verb::GatherDigits(Gather {
                min: optional_u32(value, "min")?.unwrap_or(0),
                max: optional_u32(value, "max")?,
                terminators: value
                    .get("terminators")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                digit_timeout_ms: optional_u32(value, "digit_timeout_ms")?,
                timeout_ms: optional_u32(value, "timeout_ms")?,
                prompt: value.get("prompt").map(Source::from_json).transpose()?,
            }),
            "record" => Verb::Record {
                max_ms: optional_u32(value, "max_ms")?,
                idle_stop_ms: optional_u32(value, "idle_stop_ms")?,
            },
            "send_dtmf" => Verb::SendDtmf {
                digits: string_field(value, "digits")?,
                duration_ms: optional_u32(value, "duration_ms")?,
            },
            _ => return Ok(None),
        }))
    }

    /// §6.2's verbs that create, join or redirect a second leg.
    fn leg_verb(name: &str, value: &Json) -> Result<Option<Verb>> {
        Ok(Some(match name {
            "dial" => Verb::Dial {
                target: string_field(value, "target")?,
                from: value.get("from").and_then(Json::as_str).map(str::to_owned),
                timeout_ms: optional_u32(value, "timeout_ms")?,
                headers: string_map(value.get("headers")),
            },
            "bridge" => Verb::Bridge {
                leg: string_field(value, "leg")?,
                dtmf: match value.get("dtmf").and_then(Json::as_str) {
                    Some(text) => DtmfMode::parse(text).ok_or(Error::BadField { field: "dtmf" })?,
                    None => DtmfMode::default(),
                },
            },
            "unbridge" => Verb::Unbridge,
            "transfer" => Verb::Transfer {
                target: match (
                    value.get("target").and_then(Json::as_str),
                    value.get("via_leg").and_then(Json::as_str),
                ) {
                    // §6.2 says `target` **or** `via_leg`. Both at once has no defined reading, so
                    // it is refused rather than resolved by a precedence nobody wrote down.
                    (Some(_), Some(_)) | (None, None) => {
                        return Err(Error::BadField { field: "target" });
                    }
                    (Some(target), None) => TransferTarget::Blind {
                        target: target.to_owned(),
                    },
                    (None, Some(via_leg)) => TransferTarget::Attended {
                        via_leg: via_leg.to_owned(),
                    },
                },
            },
            "accept_transfer" => Verb::AcceptTransfer,
            "refuse_transfer" => Verb::RefuseTransfer {
                status: match value.get("status") {
                    Some(_) => status_field(value, "status")?,
                    None => 603,
                },
            },
            _ => return Ok(None),
        }))
    }

    /// §3's interpreter-internal verbs: a timer and a snapshot write, neither of which reaches
    /// the call framework.
    fn internal_verb(name: &str, value: &Json) -> Result<Option<Verb>> {
        Ok(Some(match name {
            "pause" => Verb::Pause {
                ms: optional_u32(value, "ms")?.ok_or(Error::MissingField { field: "ms" })?,
            },
            "tag" => Verb::Tag {
                key: string_field(value, "key")?,
                value: string_field(value, "value")?,
            },
            _ => return Ok(None),
        }))
    }

    fn from_json(value: &Json) -> Result<Self> {
        let name = value
            .get("do")
            .and_then(Json::as_str)
            .ok_or(Error::MissingField { field: "do" })?;
        let verb = if let Some(verb) = Self::control_verb(name, value)? {
            verb
        } else if let Some(verb) = Self::media_verb(name, value)? {
            verb
        } else if let Some(verb) = Self::leg_verb(name, value)? {
            verb
        } else if let Some(verb) = Self::internal_verb(name, value)? {
            verb
        } else {
            // §4: an unknown verb is an error, never a skip. A host that ran the rest of the
            // document would be running a different program than the app wrote.
            return Err(Error::UnknownVerb {
                verb: name.to_owned(),
            });
        };
        Ok(Self {
            id: string_field(value, "id")?,
            verb,
        })
    }
}

/// A whole response document (§6.1) — which is the app's *entire* new program (§6.3).
///
/// There is no way to build one of these from a body that had anything wrong with it: §6.4's
/// "rejected whole" is [`Document::parse`]'s signature, not a rule the caller has to remember.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    /// The instructions, in the order they run. Empty is valid and means "keep going" (§6.3).
    pub instructions: Vec<Instruction>,
}

impl Document {
    /// A document of these instructions.
    #[must_use]
    pub fn new(instructions: Vec<Instruction>) -> Self {
        Self { instructions }
    }

    /// The empty document — §6.3's "keep going".
    #[must_use]
    pub fn keep_going() -> Self {
        Self::default()
    }

    /// This document as JSON text.
    #[must_use]
    pub fn to_text(&self) -> String {
        Json::object([
            ("contract", Some(Json::Str(CONTRACT.to_owned()))),
            (
                "instructions",
                Some(Json::Array(
                    self.instructions.iter().map(Instruction::to_json).collect(),
                )),
            ),
        ])
        .to_text()
    }

    /// Read a document the app sent.
    ///
    /// This is the crate's hostile-input boundary: the bytes come from somebody else's process
    /// over somebody else's network. It never panics, on any input.
    ///
    /// # Errors
    ///
    /// §6.4: any of these rejects the document **whole**, with no partial application — it is not
    /// JSON, it names another wire line, `instructions` is absent or is not an array, a verb is
    /// unknown, a field has no valid reading, or two instructions share an id.
    pub fn parse(text: &str) -> Result<Self> {
        let value = Json::parse(text)?;
        check_contract(&value)?;
        let items =
            value
                .get("instructions")
                .and_then(Json::as_array)
                .ok_or(Error::MissingField {
                    field: "instructions",
                })?;
        let instructions = items
            .iter()
            .map(Instruction::from_json)
            .collect::<Result<Vec<_>>>()?;
        // §6.1: ids are unique within the call, because they are what completion events correlate
        // against. Two with one id makes that correlation ambiguous, so the document is refused
        // rather than resolved by picking one of them.
        let mut seen: Vec<&str> = Vec::with_capacity(instructions.len());
        for instruction in &instructions {
            if seen.contains(&instruction.id.as_str()) {
                return Err(Error::DuplicateId {
                    id: instruction.id.clone(),
                });
            }
            seen.push(&instruction.id);
        }
        Ok(Self { instructions })
    }
}

fn status_field(value: &Json, field: &'static str) -> Result<u16> {
    let raw = value
        .get(field)
        .and_then(Json::as_i64)
        .ok_or(Error::MissingField { field })?;
    u16::try_from(raw).map_err(|_| Error::BadField { field })
}

fn optional_u32(value: &Json, field: &'static str) -> Result<Option<u32>> {
    match value.get(field) {
        None => Ok(None),
        Some(found) => {
            let raw = found.as_i64().ok_or(Error::BadField { field })?;
            u32::try_from(raw)
                .map(Some)
                .map_err(|_| Error::BadField { field })
        }
    }
}

fn string_map(value: Option<&Json>) -> BTreeMap<String, String> {
    value
        .and_then(Json::as_object)
        .map(|members| {
            members
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// §6.1's own example document, read as written.
    #[test]
    fn the_spec_s_example_document_parses_as_written() {
        let text = r##"{
          "contract": "sipx.app.v1",
          "instructions": [
            { "id": "p1", "do": "play", "source": { "file": "welcome.wav" }, "interruptible": true },
            { "id": "g1", "do": "gather", "max": 4, "terminators": "#", "digit_timeout_ms": 4000, "timeout_ms": 10000 }
          ]
        }"##;
        let document = Document::parse(text).unwrap();
        assert_eq!(document.instructions.len(), 2);
        assert_eq!(document.instructions[0].id, "p1");
        assert_eq!(
            document.instructions[0].verb,
            Verb::Play {
                source: Source::File("welcome.wav".to_owned()),
                interruptible: true,
            }
        );
        assert_eq!(
            document.instructions[1].verb,
            Verb::GatherDigits(Gather {
                min: 0,
                max: Some(4),
                terminators: "#".to_owned(),
                digit_timeout_ms: Some(4_000),
                timeout_ms: Some(10_000),
                prompt: None,
            })
        );
    }

    /// §6.4: unknown verb, rejected whole — not "the other three instructions still ran".
    #[test]
    fn an_unknown_verb_rejects_the_whole_document() {
        let text = r#"{"contract":"sipx.app.v1","instructions":[
            {"id":"a","do":"answer"},
            {"id":"s","do":"spindle"},
            {"id":"h","do":"hangup"}]}"#;
        assert_eq!(
            Document::parse(text),
            Err(Error::UnknownVerb {
                verb: "spindle".to_owned()
            })
        );
    }

    #[test]
    fn two_instructions_may_not_share_an_id() {
        let text = r#"{"contract":"sipx.app.v1","instructions":[
            {"id":"x","do":"answer"},{"id":"x","do":"hangup"}]}"#;
        assert_eq!(
            Document::parse(text),
            Err(Error::DuplicateId { id: "x".to_owned() })
        );
    }

    /// §6.5: there is no URL source in `v1`, and a document that names one is refused rather
    /// than quietly treated as a filename.
    #[test]
    fn a_url_source_is_not_a_source() {
        let text = r#"{"contract":"sipx.app.v1","instructions":[
            {"id":"p","do":"play","source":{"url":"https://example.net/a.wav"}}]}"#;
        assert_eq!(
            Document::parse(text),
            Err(Error::BadField { field: "source" })
        );
    }

    /// §6.2: `target` **or** `via_leg`; neither reading exists for "both" or "neither".
    #[test]
    fn a_transfer_names_exactly_one_kind_of_target() {
        let both = r#"{"contract":"sipx.app.v1","instructions":[
            {"id":"t","do":"transfer","target":"sip:c@e.net","via_leg":"b"}]}"#;
        let neither = r#"{"contract":"sipx.app.v1","instructions":[{"id":"t","do":"transfer"}]}"#;
        for text in [both, neither] {
            assert_eq!(
                Document::parse(text),
                Err(Error::BadField { field: "target" })
            );
        }
    }

    #[test]
    fn an_empty_document_is_valid_and_means_keep_going() {
        let document = Document::parse(r#"{"contract":"sipx.app.v1","instructions":[]}"#).unwrap();
        assert_eq!(document, Document::keep_going());
    }

    #[test]
    fn every_verb_survives_a_round_trip() {
        let instructions = crate::testing::one_of_every_verb();
        assert_eq!(instructions.len(), Verb::names().len());
        let document = Document::new(instructions);
        let text = document.to_text();
        assert_eq!(Document::parse(&text), Ok(document), "from {text}");
    }

    #[test]
    fn nothing_a_peer_can_send_as_a_document_panics() {
        for text in crate::testing::HOSTILE_BODIES {
            let _ = Document::parse(text);
        }
    }
}
