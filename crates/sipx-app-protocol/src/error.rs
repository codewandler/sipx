//! What can be wrong with a document or an envelope.

use std::fmt;

use crate::json::JsonError;

/// A `sipx.app.v1` document or envelope this crate cannot read.
///
/// §6.4 of the contract says a document with any of these problems is rejected **whole** — there
/// is no partial application — so this type is deliberately a single flat reason rather than a
/// list: the first thing wrong with a document is the only thing that matters about it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// It is not JSON (RFC 8259).
    Json(JsonError),
    /// The `contract` member is missing, or names a line this crate does not speak.
    WrongContract {
        /// What was there, if anything.
        found: Option<String>,
    },
    /// A member the vocabulary requires is absent.
    MissingField {
        /// Which one.
        field: &'static str,
    },
    /// A member is present with a value the vocabulary has no reading for.
    BadField {
        /// Which one.
        field: &'static str,
    },
    /// §6.4: a verb this contract does not define. Never ignored — a host that skipped a verb it
    /// did not know would run a different program than the app wrote.
    UnknownVerb {
        /// The verb as it was spelled.
        verb: String,
    },
    /// §6.1: instruction `id`s are unique within a call, because they are what the completion
    /// events correlate against. Two instructions with one id make that correlation ambiguous.
    DuplicateId {
        /// The id used twice.
        id: String,
    },
}

impl From<JsonError> for Error {
    fn from(error: JsonError) -> Self {
        Self::Json(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "not JSON: {error}"),
            Self::WrongContract { found: Some(line) } => {
                write!(f, "contract line {line:?} is not {}", crate::CONTRACT)
            }
            Self::WrongContract { found: None } => {
                write!(f, "no contract line; {} was expected", crate::CONTRACT)
            }
            Self::MissingField { field } => write!(f, "missing field {field:?}"),
            Self::BadField { field } => write!(f, "field {field:?} has no valid reading"),
            Self::UnknownVerb { verb } => write!(f, "unknown instruction verb {verb:?}"),
            Self::DuplicateId { id } => write!(f, "instruction id {id:?} is used twice"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

/// The result of reading something off the wire.
pub type Result<T> = std::result::Result<T, Error>;
