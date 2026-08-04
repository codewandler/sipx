//! Full-duplex session frames.
//!
//! [`docs/specs/session-binding.md`](../../../docs/specs/session-binding.md) fixes the wrapper that
//! lets one WebSocket carry many calls. Call events remain ordinary [`Envelope`](crate::Envelope)s;
//! these are the app-to-host commands and their correlated host replies.

use crate::document::Document;
use crate::error::{Error, Result};
use crate::event::CONTRACT;
use crate::json::Json;

/// The largest app-chosen request correlation, in UTF-8 bytes.
pub const MAX_SESSION_REQUEST_BYTES: usize = 128;

/// One app-to-host session command.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SessionRequest {
    /// Replace one live call's program.
    Document {
        /// App-chosen correlation.
        request: String,
        /// The call pinned to this session.
        call: String,
        /// The whole replacement document.
        document: Document,
    },
    /// Place a new outbound call.
    Originate {
        /// App-chosen correlation.
        request: String,
        /// Destination SIP URI.
        target: String,
        /// Caller SIP URI.
        from: String,
    },
}

impl SessionRequest {
    /// Recover a valid request correlation even when the rest of a frame is malformed.
    #[must_use]
    pub fn correlation_from_text(text: &str) -> Option<String> {
        let value = Json::parse(text).ok()?;
        required_bounded_string(&value, "request", MAX_SESSION_REQUEST_BYTES).ok()
    }

    /// Parse one complete JSON text frame.
    ///
    /// # Errors
    ///
    /// The same typed hostile-input errors as the contract document parser. The request is
    /// rejected whole; no partially parsed command is returned.
    pub fn parse(text: &str) -> Result<Self> {
        let value = Json::parse(text)?;
        let found = value
            .get("contract")
            .and_then(Json::as_str)
            .map(ToOwned::to_owned);
        if found.as_deref() != Some(CONTRACT) {
            return Err(Error::WrongContract { found });
        }
        let request = required_bounded_string(&value, "request", MAX_SESSION_REQUEST_BYTES)?;
        if value.get("do").is_some() {
            if value.get("do").and_then(Json::as_str) != Some("originate") {
                return Err(Error::BadField { field: "do" });
            }
            return Ok(Self::Originate {
                request,
                target: required_string(&value, "target")?,
                from: required_string(&value, "from")?,
            });
        }
        Ok(Self::Document {
            request,
            call: required_string(&value, "call")?,
            document: Document::parse(text)?,
        })
    }

    /// The app-chosen correlation.
    #[must_use]
    pub fn correlation(&self) -> &str {
        match self {
            Self::Document { request, .. } | Self::Originate { request, .. } => request,
        }
    }
}

fn required_string(value: &Json, field: &'static str) -> Result<String> {
    value
        .get(field)
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
        .ok_or(Error::MissingField { field })
}

fn required_bounded_string(value: &Json, field: &'static str, max: usize) -> Result<String> {
    let text = required_string(value, field)?;
    if text.len() > max {
        return Err(Error::BadField { field });
    }
    Ok(text)
}

/// Machine-readable session error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionErrorCode {
    /// The text frame is not a valid session command.
    BadFrame,
    /// The call is absent, ended, or pinned to another session.
    UnknownCall,
    /// The per-call command queue is full.
    CallBusy,
    /// The app lacks the originate grant.
    OriginateForbidden,
    /// The call could not be placed.
    OriginateFailed,
}

impl SessionErrorCode {
    /// Wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BadFrame => "bad_frame",
            Self::UnknownCall => "unknown_call",
            Self::CallBusy => "call_busy",
            Self::OriginateForbidden => "originate_forbidden",
            Self::OriginateFailed => "originate_failed",
        }
    }
}

/// One correlated host-to-app command reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionReply {
    request: Option<String>,
    body: ReplyBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplyBody {
    Result {
        call: String,
    },
    Error {
        code: SessionErrorCode,
        message: String,
    },
}

impl SessionReply {
    /// A successful document or originate result.
    #[must_use]
    pub fn result(request: impl Into<String>, call: impl Into<String>) -> Self {
        Self {
            request: Some(request.into()),
            body: ReplyBody::Result { call: call.into() },
        }
    }

    /// A typed failure. `request` is absent only if the frame yielded no valid correlation.
    #[must_use]
    pub fn error(
        request: Option<impl Into<String>>,
        code: SessionErrorCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            request: request.map(Into::into),
            body: ReplyBody::Error {
                code,
                message: message.into(),
            },
        }
    }

    /// Compact JSON text.
    #[must_use]
    pub fn to_text(&self) -> String {
        let (result, error) = match &self.body {
            ReplyBody::Result { call } => (
                Some(Json::object([("call", Some(Json::Str(call.clone())))])),
                None,
            ),
            ReplyBody::Error { code, message } => (
                None,
                Some(Json::object([
                    ("code", Some(Json::Str(code.as_str().to_owned()))),
                    ("message", Some(Json::Str(message.clone()))),
                ])),
            ),
        };
        Json::object([
            ("contract", Some(Json::Str(CONTRACT.to_owned()))),
            ("request", self.request.clone().map(Json::Str)),
            ("result", result),
            ("error", error),
        ])
        .to_text()
    }
}
