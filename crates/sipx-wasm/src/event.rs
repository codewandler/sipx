//! Events, kernel → host (`docs/specs/browser-sdk.md` §5.3).
//!
//! Call and registration events are **snapshots, not deltas**: each one replaces the previous
//! state wholesale, so a missed delivery cannot leave the page permanently wrong.
//!
//! Field order here is the order of §5.3's table, and it is load-bearing: `BSDK-EVT-1`,
//! `BSDK-EVT-2` and `BSDK-EVT-3` are pinned by SHA-256 over exactly these bytes.

use crate::command::MediaKind;
use crate::json::Writer;

/// Registration state, replaced wholesale on every change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegistrationState {
    Registering,
    Registered,
    Unregistered,
    Failed,
}

impl RegistrationState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Registering => "registering",
            Self::Registered => "registered",
            Self::Unregistered => "unregistered",
            Self::Failed => "failed",
        }
    }
}

/// Which side started the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    In,
    Out,
}

impl Direction {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
        }
    }
}

/// Why a call ended (§5.3's `"cause"` object).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cause {
    pub(crate) class: CauseClass,
    pub(crate) status: Option<u64>,
    pub(crate) reason: Option<String>,
}

impl Cause {
    pub(crate) fn class(class: CauseClass) -> Self {
        Self {
            class,
            status: None,
            reason: None,
        }
    }

    pub(crate) fn sip(status: u64, reason: impl Into<String>) -> Self {
        Self {
            class: CauseClass::Sip,
            status: Some(status),
            reason: Some(reason.into()),
        }
    }

    pub(crate) fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

/// The six terminal classes. A media failure never presents as a SIP failure and vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CauseClass {
    Local,
    Remote,
    Refused,
    Sip,
    Media,
    Timeout,
}

impl CauseClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::Refused => "refused",
            Self::Sip => "sip",
            Self::Media => "media",
            Self::Timeout => "timeout",
        }
    }
}

/// A command's single completion (§5.2: exactly one `"outcome"` per command, at protocol
/// completion rather than at acceptance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub(crate) id: u64,
    pub(crate) error: Option<OutcomeError>,
}

/// A typed refusal carried inside an `"outcome"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutcomeError {
    pub(crate) code: &'static str,
    pub(crate) reason: String,
}

impl OutcomeError {
    pub(crate) fn new(code: &'static str, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
        }
    }
}

/// One §5.3 event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    NeedEntropy {
        min: u64,
    },
    Registration {
        state: RegistrationState,
        expires: Option<u64>,
        status: Option<u64>,
        reason: Option<String>,
    },
    Call {
        call: u32,
        dir: Direction,
        state: &'static str,
        from: Option<String>,
        to: Option<String>,
    },
    NeedLocalMedia {
        call: u32,
        kind: MediaKind,
    },
    RemoteMedia {
        call: u32,
        kind: MediaKind,
        sdp: String,
    },
    CallEnded {
        call: u32,
        cause: Cause,
    },
    Outcome(Outcome),
    Fault {
        fatal: bool,
        code: &'static str,
        reason: String,
    },
}

impl Event {
    /// Render the canonical document.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut writer = Writer::object();
        writer.number("v", 1);
        match self {
            Self::NeedEntropy { min } => {
                writer.string("evt", "need-entropy").number("min", *min);
            }
            Self::Registration {
                state,
                expires,
                status,
                reason,
            } => {
                writer
                    .string("evt", "registration")
                    .string("state", state.as_str())
                    .number_opt("expires", *expires)
                    .number_opt("status", *status)
                    .string_opt("reason", reason.as_deref());
            }
            Self::Call {
                call,
                dir,
                state,
                from,
                to,
            } => {
                writer
                    .string("evt", "call")
                    .number("call", u64::from(*call))
                    .string("dir", dir.as_str())
                    .string("state", state)
                    .string_opt("from", from.as_deref())
                    .string_opt("to", to.as_deref());
            }
            Self::NeedLocalMedia { call, kind } => {
                writer
                    .string("evt", "need-local-media")
                    .number("call", u64::from(*call))
                    .string("kind", kind.as_str())
                    // Always exactly this: the contract is audio-only, and a page that asked for
                    // video would be asking a kernel that refuses video sections outright.
                    .object_field("constraints", |constraints| {
                        constraints.boolean("audio", true).boolean("video", false);
                    });
            }
            Self::RemoteMedia { call, kind, sdp } => {
                writer
                    .string("evt", "remote-media")
                    .number("call", u64::from(*call))
                    .string("kind", kind.as_str())
                    .string("sdp", sdp);
            }
            Self::CallEnded { call, cause } => {
                writer
                    .string("evt", "call-ended")
                    .number("call", u64::from(*call))
                    .object_field("cause", |object| {
                        object
                            .string("class", cause.class.as_str())
                            .number_opt("status", cause.status)
                            .string_opt("reason", cause.reason.as_deref());
                    });
            }
            Self::Outcome(outcome) => {
                writer
                    .string("evt", "outcome")
                    .number("id", outcome.id)
                    .boolean("ok", outcome.error.is_none());
                if let Some(error) = &outcome.error {
                    writer.object_field("error", |object| {
                        object
                            .string("code", error.code)
                            .string("reason", &error.reason);
                    });
                }
            }
            Self::Fault {
                fatal,
                code,
                reason,
            } => {
                writer
                    .string("evt", "error")
                    .boolean("fatal", *fatal)
                    .string("code", code)
                    .string("reason", reason);
            }
        }
        writer.finish().into_bytes()
    }
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

    #[test]
    fn bsdk_evt_1_need_entropy() {
        let bytes = Event::NeedEntropy { min: 64 }.encode();
        assert_eq!(bytes, br#"{"v":1,"evt":"need-entropy","min":64}"#);
        assert_eq!(bytes.len(), 37);
    }

    #[test]
    fn bsdk_evt_2_registration_registered() {
        let bytes = Event::Registration {
            state: RegistrationState::Registered,
            expires: Some(600),
            status: None,
            reason: None,
        }
        .encode();
        assert_eq!(
            bytes,
            br#"{"v":1,"evt":"registration","state":"registered","expires":600}"#
        );
        assert_eq!(bytes.len(), 63);
    }

    #[test]
    fn bsdk_evt_3_need_local_media() {
        let bytes = Event::NeedLocalMedia {
            call: 1,
            kind: MediaKind::Offer,
        }
        .encode();
        assert_eq!(
            bytes,
            &br#"{"v":1,"evt":"need-local-media","call":1,"kind":"offer","constraints":{"audio":true,"video":false}}"#[..]
        );
        assert_eq!(bytes.len(), 99);
    }

    #[test]
    fn an_outcome_failure_carries_a_typed_code() {
        let bytes = Event::Outcome(Outcome {
            id: 7,
            error: Some(OutcomeError::new("call-limit", "eight concurrent calls")),
        })
        .encode();
        assert_eq!(
            bytes,
            &br#"{"v":1,"evt":"outcome","id":7,"ok":false,"error":{"code":"call-limit","reason":"eight concurrent calls"}}"#[..]
        );
    }

    #[test]
    fn a_successful_outcome_has_no_error_object() {
        let bytes = Event::Outcome(Outcome { id: 1, error: None }).encode();
        assert_eq!(bytes, &br#"{"v":1,"evt":"outcome","id":1,"ok":true}"#[..]);
    }
}
