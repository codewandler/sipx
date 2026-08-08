//! Commands, host → kernel (`docs/specs/browser-sdk.md` §5.1, §5.2).
//!
//! Ten verbs and no more: §10 says "the v1 vocabulary is exactly §5.2", so an unknown verb is
//! `E_SCHEMA` rather than something to skip. A kernel that ignored a verb would run a different
//! program than the page wrote. Unknown *fields* are ignored, which is what makes an additive
//! field a compatible change.

use serde_json::{Map, Value};

use crate::bounds;
use crate::error::{Error, Result};

/// Which direction a description was authored in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaKind {
    /// An RFC 3264 offer.
    Offer,
    /// An RFC 3264 answer.
    Answer,
}

impl MediaKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Offer => "offer",
            Self::Answer => "answer",
        }
    }
}

/// One §5.2 verb with its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verb {
    Register {
        expires: u32,
    },
    Unregister,
    Dial {
        target: String,
    },
    Ring {
        call: u32,
    },
    Answer {
        call: u32,
    },
    Reject {
        call: u32,
        status: u16,
    },
    Hangup {
        call: u32,
    },
    LocalMedia {
        call: u32,
        kind: MediaKind,
        sdp: String,
    },
    MediaApplied {
        call: u32,
    },
    MediaFailed {
        call: u32,
        reason: String,
    },
}

/// A parsed command: the envelope's `"id"` and the verb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Command {
    pub(crate) id: u64,
    pub(crate) verb: Verb,
}

impl Command {
    /// Parse one command document.
    ///
    /// The §4.9 length bound is checked by the caller **before** this runs, because §9.5's
    /// `BSDK-NEG-7` requires `E_BOUNDS` *before JSON parsing* — a 32 KiB budget is not a budget
    /// if an oversize document still gets parsed to find out.
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        debug_assert!(bytes.len() <= bounds::MAX_COMMAND);
        let text = core::str::from_utf8(bytes).map_err(|_| Error::Utf8)?;
        let document: Value = serde_json::from_str(text).map_err(|_| Error::Json)?;
        let root = object(&document)?;
        require_version(root)?;

        let id = field(root, "id")?.as_u64().ok_or(Error::Schema)?;
        if id == 0 {
            return Err(Error::Schema);
        }

        let verb = match field(root, "cmd")?.as_str().ok_or(Error::Schema)? {
            "register" => Verb::Register {
                expires: u32::try_from(field(root, "expires")?.as_u64().ok_or(Error::Schema)?)
                    .map_err(|_| Error::Schema)
                    .and_then(|expires| {
                        if expires >= 1 {
                            Ok(expires)
                        } else {
                            Err(Error::Schema)
                        }
                    })?,
            },
            "unregister" => Verb::Unregister,
            "dial" => Verb::Dial {
                target: sip_uri(field(root, "target")?)?,
            },
            "ring" => Verb::Ring { call: call(root)? },
            "answer" => Verb::Answer { call: call(root)? },
            "reject" => {
                let status = field(root, "status")?.as_u64().ok_or(Error::Schema)?;
                if !(300..=699).contains(&status) {
                    return Err(Error::Schema);
                }
                Verb::Reject {
                    call: call(root)?,
                    status: u16::try_from(status).map_err(|_| Error::Schema)?,
                }
            }
            "hangup" => Verb::Hangup { call: call(root)? },
            "local-media" => Verb::LocalMedia {
                call: call(root)?,
                kind: match field(root, "kind")?.as_str().ok_or(Error::Schema)? {
                    "offer" => MediaKind::Offer,
                    "answer" => MediaKind::Answer,
                    _ => return Err(Error::Schema),
                },
                sdp: field(root, "sdp")?
                    .as_str()
                    .ok_or(Error::Schema)?
                    .to_owned(),
            },
            "media-applied" => Verb::MediaApplied { call: call(root)? },
            "media-failed" => Verb::MediaFailed {
                call: call(root)?,
                reason: field(root, "reason")?
                    .as_str()
                    .ok_or(Error::Schema)?
                    .to_owned(),
            },
            _ => return Err(Error::Schema),
        };

        Ok(Self { id, verb })
    }
}

/// A document's root object, or `E_SCHEMA`.
pub(crate) fn object(value: &Value) -> Result<&Map<String, Value>> {
    value.as_object().ok_or(Error::Schema)
}

/// A required field, or `E_SCHEMA`.
pub(crate) fn field<'a>(root: &'a Map<String, Value>, name: &str) -> Result<&'a Value> {
    root.get(name).ok_or(Error::Schema)
}

/// `"v":1`, or `E_SCHEMA`. A different wire line is a different program.
pub(crate) fn require_version(root: &Map<String, Value>) -> Result<()> {
    if field(root, "v")?.as_u64() == Some(1) {
        Ok(())
    } else {
        Err(Error::Schema)
    }
}

fn call(root: &Map<String, Value>) -> Result<u32> {
    let number = field(root, "call")?.as_u64().ok_or(Error::Schema)?;
    u32::try_from(number).map_err(|_| Error::Schema)
}

/// A field that must hold a parseable SIP URI.
fn sip_uri(value: &Value) -> Result<String> {
    let raw = value.as_str().ok_or(Error::Schema)?;
    let _ = sipx_sip::Uri::parse(bytes::Bytes::from(raw.to_owned().into_bytes()))
        .map_err(|_| Error::Schema)?;
    Ok(raw.to_owned())
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

    /// `BSDK-CMD-1`, `BSDK-CMD-2` and `BSDK-CMD-3` from §9.2, byte for byte.
    pub(crate) const BSDK_CMD_1: &[u8] = br#"{"v":1,"cmd":"register","id":1,"expires":600}"#;
    pub(crate) const BSDK_CMD_2: &[u8] =
        br#"{"v":1,"cmd":"dial","id":2,"target":"sip:bob@example.net"}"#;
    pub(crate) const BSDK_CMD_3: &[u8] = br#"{"v":1,"cmd":"hangup","id":3,"call":1}"#;

    #[test]
    fn the_three_command_vectors_have_their_stated_lengths() {
        assert_eq!(BSDK_CMD_1.len(), 45);
        assert_eq!(BSDK_CMD_2.len(), 58);
        assert_eq!(BSDK_CMD_3.len(), 38);
    }

    #[test]
    fn bsdk_cmd_1_is_a_register_for_six_hundred_seconds() {
        assert_eq!(
            Command::parse(BSDK_CMD_1).unwrap(),
            Command {
                id: 1,
                verb: Verb::Register { expires: 600 },
            }
        );
    }

    #[test]
    fn bsdk_cmd_2_is_a_dial() {
        assert_eq!(
            Command::parse(BSDK_CMD_2).unwrap(),
            Command {
                id: 2,
                verb: Verb::Dial {
                    target: "sip:bob@example.net".to_owned()
                },
            }
        );
    }

    #[test]
    fn bsdk_cmd_3_is_a_hangup_naming_call_one() {
        assert_eq!(
            Command::parse(BSDK_CMD_3).unwrap(),
            Command {
                id: 3,
                verb: Verb::Hangup { call: 1 },
            }
        );
    }

    #[test]
    fn bsdk_neg_4_a_truncated_document_is_json_not_schema() {
        assert_eq!(Command::parse(br#"{"v":1,"cmd":"#), Err(Error::Json));
    }

    #[test]
    fn bsdk_neg_5_an_unlisted_verb_is_schema() {
        assert_eq!(
            Command::parse(br#"{"v":1,"cmd":"transfer","id":9}"#),
            Err(Error::Schema)
        );
    }

    #[test]
    fn invalid_utf8_is_reported_before_json() {
        assert_eq!(Command::parse(&[0x7b, 0xff, 0x7d]), Err(Error::Utf8));
    }

    #[test]
    fn unknown_fields_are_ignored_within_wire_line_one() {
        let document = br#"{"v":1,"cmd":"register","id":1,"expires":600,"future":{"x":[1]}}"#;
        assert_eq!(
            Command::parse(document).unwrap().verb,
            Verb::Register { expires: 600 }
        );
    }

    #[test]
    fn a_wrong_wire_line_is_refused() {
        assert_eq!(
            Command::parse(br#"{"v":2,"cmd":"unregister","id":1}"#),
            Err(Error::Schema)
        );
    }

    #[test]
    fn register_below_one_second_is_refused() {
        assert_eq!(
            Command::parse(br#"{"v":1,"cmd":"register","id":1,"expires":0}"#),
            Err(Error::Schema)
        );
    }

    #[test]
    fn reject_outside_the_final_response_range_is_refused() {
        assert_eq!(
            Command::parse(br#"{"v":1,"cmd":"reject","id":1,"call":1,"status":180}"#),
            Err(Error::Schema)
        );
        assert_eq!(
            Command::parse(br#"{"v":1,"cmd":"reject","id":1,"call":1,"status":486}"#)
                .unwrap()
                .verb,
            Verb::Reject {
                call: 1,
                status: 486
            }
        );
    }

    #[test]
    fn a_dial_target_that_is_not_a_sip_uri_is_refused() {
        assert_eq!(
            Command::parse(br#"{"v":1,"cmd":"dial","id":2,"target":"not a uri"}"#),
            Err(Error::Schema)
        );
    }

    #[test]
    fn a_zero_command_id_is_refused() {
        assert_eq!(
            Command::parse(br#"{"v":1,"cmd":"unregister","id":0}"#),
            Err(Error::Schema)
        );
    }
}
