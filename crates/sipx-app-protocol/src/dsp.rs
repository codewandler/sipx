//! The DSP vocabulary an application composes a graph out of, app → host and host → app.
//!
//! [`docs/specs/app-contract.md`](../../../../docs/specs/app-contract.md) §6.2's four `dsp` verbs
//! and §5.3's six `call.dsp.*` events, plus the values they carry. The normative graph behind them
//! is [`docs/specs/call-dsp-graph.md`](../../../../docs/specs/call-dsp-graph.md) §10.
//!
//! **What this vocabulary can say.** An ordered list of processor *identifiers*, a shape where a
//! processor declares one, and a finite parameter set per stage. That is all of it.
//!
//! **What it structurally cannot say**, in the same sense that §6.5's [`Source`](crate::Source) has
//! no URL variant: a processor, a program, a callback, an execution profile, a deadline, a failure
//! action, a graph bound, or a containment claim. None of those has a shape here, so refusing them
//! is not a check an implementation performs — there is nothing to refuse. A host resolves an
//! identifier against its own registry, and an identifier that resolves to nothing is refused with
//! [`DspRefusal::UnknownProcessor`] rather than into something plausible nearby.
//!
//! **Why the value kinds are spelled out.** `docs/specs/custom-call-dsp.md` §3.6's parameter
//! vocabulary is `Flag`, `Integer` and `Ratio`, and the last two are both JSON numbers. Writing
//! `{"ratio": 2000}` rather than `2000` makes the kind part of what the application said, so a
//! thousandths value sent where an integer was declared is that contract's own `KindMismatch`
//! rather than a number this crate had to guess the meaning of. There is no floating-point
//! parameter, here or anywhere in that contract.

use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::json::Json;

/// One value in a parameter set (`docs/specs/custom-call-dsp.md` §3.6).
///
/// Finite by construction: there is no floating-point variant, so a NaN gain, an infinite cutoff
/// and a payload smuggled through a bit pattern are unrepresentable rather than refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DspValue {
    /// Either boolean.
    Flag(bool),
    /// A whole number, held to the parameter's declared inclusive range.
    Integer(i64),
    /// Thousandths of a unit, held to the parameter's declared inclusive range.
    Ratio(i32),
}

impl DspValue {
    /// Its `{"kind": value}` shape on the wire.
    #[must_use]
    pub fn to_json(self) -> Json {
        match self {
            Self::Flag(flag) => Json::object([("flag", Some(Json::from(flag)))]),
            Self::Integer(value) => Json::object([("integer", Some(Json::Int(value)))]),
            Self::Ratio(value) => Json::object([("ratio", Some(Json::Int(i64::from(value))))]),
        }
    }

    fn from_json(value: &Json) -> Result<Self> {
        if let Some(flag) = value.get("flag").and_then(Json::as_bool) {
            return Ok(Self::Flag(flag));
        }
        if let Some(found) = value.get("integer").and_then(Json::as_i64) {
            return Ok(Self::Integer(found));
        }
        if let Some(found) = value.get("ratio").and_then(Json::as_i64) {
            return i32::try_from(found)
                .map(Self::Ratio)
                .map_err(|_| Error::BadField { field: "ratio" });
        }
        // A bare number would have to be read as an integer or as thousandths, and this contract
        // does not choose between two readings of the same text.
        Err(Error::BadField { field: "value" })
    }
}

/// One parameter of a set: the identifier the processor declares, and a value for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DspParameter {
    /// The identifier, as the processor's published schema spells it.
    pub id: String,
    /// The value.
    pub value: DspValue,
}

impl DspParameter {
    /// A parameter.
    pub fn new(id: impl Into<String>, value: DspValue) -> Self {
        Self {
            id: id.into(),
            value,
        }
    }
}

/// One stage of a chain an application composed
/// (`docs/specs/call-dsp-graph.md` §10.1).
///
/// A name, a shape and a parameter set — never code and never a profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DspStage {
    /// The registered processor's stable identifier. A name the host's registry does not publish
    /// is [`DspRefusal::UnknownProcessor`], and the whole instruction is refused.
    pub id: String,
    /// The one property a parameter cannot express: a processor's *shape*, which the graph sizes
    /// buffers from before the first frame. `0` for the processors that declare none, which today
    /// is every registered processor but the delay line.
    pub shape: u32,
    /// The finite parameter set, validated against the processor's declared schema by the host.
    pub parameters: Vec<DspParameter>,
}

impl DspStage {
    /// A stage naming a registered processor, with no shape and no parameters.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            shape: 0,
            parameters: Vec::new(),
        }
    }

    /// The same stage with a shape.
    #[must_use]
    pub const fn with_shape(mut self, shape: u32) -> Self {
        self.shape = shape;
        self
    }

    /// The same stage with one more parameter.
    #[must_use]
    pub fn with_parameter(mut self, id: impl Into<String>, value: DspValue) -> Self {
        self.parameters.push(DspParameter::new(id, value));
        self
    }

    /// This stage as JSON.
    #[must_use]
    pub fn to_json(&self) -> Json {
        Json::object([
            ("id", Some(Json::Str(self.id.clone()))),
            ("shape", Some(Json::from(self.shape))),
            ("parameters", Some(parameters_to_json(&self.parameters))),
        ])
    }

    fn from_json(value: &Json) -> Result<Self> {
        Ok(Self {
            id: crate::event::string_field(value, "id")?,
            shape: match value.get("shape") {
                None => 0,
                Some(found) => {
                    let raw = found.as_i64().ok_or(Error::BadField { field: "shape" })?;
                    u32::try_from(raw).map_err(|_| Error::BadField { field: "shape" })?
                }
            },
            parameters: parameters_from_json(value.get("parameters"))?,
        })
    }
}

/// A parameter set as a JSON object keyed by identifier.
pub(crate) fn parameters_to_json(parameters: &[DspParameter]) -> Json {
    Json::Object(
        parameters
            .iter()
            .map(|parameter| (parameter.id.clone(), parameter.value.to_json()))
            .collect(),
    )
}

/// Read a parameter set back. An absent member is the empty set, which is a valid thing to say.
pub(crate) fn parameters_from_json(value: Option<&Json>) -> Result<Vec<DspParameter>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let members: &BTreeMap<String, Json> = value.as_object().ok_or(Error::BadField {
        field: "parameters",
    })?;
    members
        .iter()
        .map(|(id, raw)| {
            Ok(DspParameter {
                id: id.clone(),
                value: DspValue::from_json(raw)?,
            })
        })
        .collect()
}

/// Read an ordered chain back.
pub(crate) fn stages_from_json(value: Option<&Json>) -> Result<Vec<DspStage>> {
    let items = value.and_then(Json::as_array).ok_or(Error::MissingField {
        field: "processors",
    })?;
    items.iter().map(DspStage::from_json).collect()
}

/// Why one stage stopped contributing (§5.3; `call-dsp-graph.md` §5.3's `BypassCause`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DspBypassCause {
    /// The application asked for it.
    Requested,
    /// The stage refused its frames until it had failed under the configured miss budget.
    Refused,
    /// No result was present by the configured deadline.
    DeadlineMissed,
    /// A stage answered with a position count other than the frame's.
    MalformedResult,
    /// A supervised worker failed terminally, or its channel closed.
    WorkerLost,
}

impl DspBypassCause {
    /// Its spelling on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Refused => "refused",
            Self::DeadlineMissed => "deadline_missed",
            Self::MalformedResult => "malformed_result",
            Self::WorkerLost => "worker_lost",
        }
    }

    /// Every value §5.3's row lists, in the row's order.
    #[must_use]
    pub const fn all() -> [Self; 5] {
        [
            Self::Requested,
            Self::Refused,
            Self::DeadlineMissed,
            Self::MalformedResult,
            Self::WorkerLost,
        ]
    }

    fn parse(text: &str) -> Option<Self> {
        Self::all().into_iter().find(|c| c.as_str() == text)
    }
}

/// Why a graph ended (§5.3; `call-dsp-graph.md` §5.3's `TeardownCause`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DspTeardownCause {
    /// The application asked for it.
    Requested,
    /// The handle was released.
    Detached,
    /// The call's media stopped.
    SessionStopped,
    /// A stage configured fail-closed failed, so the audio it was protecting does not flow. The
    /// event's `processor` names it — unprocessed audio leaving the stack was the thing that stage
    /// existed to prevent, so the direction is silenced rather than carrying it.
    FailedClosed,
}

impl DspTeardownCause {
    /// Its spelling on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Detached => "detached",
            Self::SessionStopped => "session_stopped",
            Self::FailedClosed => "failed_closed",
        }
    }

    /// Every value §5.3's row lists, in the row's order.
    #[must_use]
    pub const fn all() -> [Self; 4] {
        [
            Self::Requested,
            Self::Detached,
            Self::SessionStopped,
            Self::FailedClosed,
        ]
    }

    fn parse(text: &str) -> Option<Self> {
        Self::all().into_iter().find(|c| c.as_str() == text)
    }
}

/// Why a `dsp`, `dsp_param`, `dsp_bypass` or `dsp_remove` instruction was refused (§5.3).
///
/// Every one of these leaves the active graph exactly as it was. That is the whole point of the
/// vocabulary: a refusal is a thing the application did wrong, never a thing the call now has to
/// live with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DspRefusal {
    /// A processor identifier the host's registry does not publish.
    UnknownProcessor,
    /// A parameter identifier outside the processor's declared schema, or a value of the wrong
    /// kind. The **whole** set is refused and the previous one stays in force.
    UnknownParameter,
    /// A value outside its parameter's declared inclusive range.
    OutOfRange,
    /// The named generation is not the one now live — the graph was replaced, or never existed.
    StaleGeneration,
    /// The chain is longer than the host's configured ceiling, or a stage is outside its bounds.
    TooManyProcessors,
    /// This direction has no graph to change or remove.
    NoGraph,
    /// A stage whose parameters cannot be moved while it runs.
    NotConfigurable,
    /// The named stage is already in the requested bypass state.
    BypassUnchanged,
    /// A supervised stage cannot leave its deadline pipeline without misaligning its audio.
    NotBypassable,
    /// The runtime bypassed this failing stage terminally; only graph replacement can restore it.
    BypassNotReversible,
    /// The host refused the plan for a reason with no narrower word here. The operator's log is
    /// where the detail belongs; a vocabulary that grew a word per internal refusal would be
    /// reporting the host's diagnosis rather than what the application got wrong.
    Rejected,
}

impl DspRefusal {
    /// Its spelling on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownProcessor => "unknown_processor",
            Self::UnknownParameter => "unknown_parameter",
            Self::OutOfRange => "out_of_range",
            Self::StaleGeneration => "stale_generation",
            Self::TooManyProcessors => "too_many_processors",
            Self::NoGraph => "no_graph",
            Self::NotConfigurable => "not_configurable",
            Self::BypassUnchanged => "bypass_unchanged",
            Self::NotBypassable => "not_bypassable",
            Self::BypassNotReversible => "bypass_not_reversible",
            Self::Rejected => "rejected",
        }
    }

    /// Every value §5.3's row lists, in the row's order.
    #[must_use]
    pub const fn all() -> [Self; 11] {
        [
            Self::UnknownProcessor,
            Self::UnknownParameter,
            Self::OutOfRange,
            Self::StaleGeneration,
            Self::TooManyProcessors,
            Self::NoGraph,
            Self::NotConfigurable,
            Self::BypassUnchanged,
            Self::NotBypassable,
            Self::BypassNotReversible,
            Self::Rejected,
        ]
    }

    fn parse(text: &str) -> Option<Self> {
        Self::all().into_iter().find(|c| c.as_str() == text)
    }
}

/// Read a `cause` naming a bypass.
pub(crate) fn bypass_cause_field(value: &Json) -> Result<DspBypassCause> {
    DspBypassCause::parse(&crate::event::string_field(value, "cause")?)
        .ok_or(Error::BadField { field: "cause" })
}

/// Read a `cause` naming a teardown.
pub(crate) fn teardown_cause_field(value: &Json) -> Result<DspTeardownCause> {
    DspTeardownCause::parse(&crate::event::string_field(value, "cause")?)
        .ok_or(Error::BadField { field: "cause" })
}

/// Read a `reason` naming a refusal.
pub(crate) fn refusal_field(value: &Json) -> Result<DspRefusal> {
    DspRefusal::parse(&crate::event::string_field(value, "reason")?)
        .ok_or(Error::BadField { field: "reason" })
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

    /// The kind is part of what the application said, so a round trip cannot lose it.
    #[test]
    fn every_value_kind_survives_a_round_trip_with_its_kind_intact() {
        for value in [
            DspValue::Flag(true),
            DspValue::Flag(false),
            DspValue::Integer(-9),
            DspValue::Integer(i64::MAX),
            DspValue::Ratio(2_000),
            DspValue::Ratio(i32::MIN),
        ] {
            assert_eq!(DspValue::from_json(&value.to_json()), Ok(value));
        }
    }

    /// A bare number has two readings under §3.6's vocabulary, so it has none here.
    #[test]
    fn a_bare_number_is_not_a_parameter_value() {
        assert_eq!(
            DspValue::from_json(&Json::Int(2_000)),
            Err(Error::BadField { field: "value" })
        );
    }

    /// There is no shape in this vocabulary for code, a program or a profile — which is why the
    /// refusal is not a check. A stage carrying one is read as the three members it does have.
    #[test]
    fn a_stage_carrying_a_profile_is_read_as_the_three_members_it_may_have() {
        let text = r#"{"id":"sipx.gain","shape":0,"parameters":{},
            "profile":"proven_inline","program":"/bin/sh","callback":"onFrame"}"#;
        let stage = DspStage::from_json(&Json::parse(text).unwrap()).unwrap();
        assert_eq!(stage, DspStage::new("sipx.gain"));
        // And what goes back out is the vocabulary, so nothing a peer invented is echoed onward.
        assert_eq!(
            stage.to_json().to_text(),
            DspStage::new("sipx.gain").to_json().to_text()
        );
    }
}
