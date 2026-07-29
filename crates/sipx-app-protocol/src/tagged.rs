//! One encoding rule for the contract's three "name, sometimes with fields" values.
//!
//! [`docs/specs/app-contract.md`](../../../../docs/specs/app-contract.md) §5.3 writes three of its
//! values the same way — `answered · busy · rejected{status} · timeout` for a dial outcome,
//! `hangup · remote · rejected{status} · timeout · error` for an end cause, `trying · ringing ·
//! succeeded · failed{status}` for transfer progress. A bare name, or a name with a brace of
//! fields after it. This module is that notation as JSON, once, so the three cannot drift apart:
//!
//! - a name with no fields is the string `"busy"`;
//! - a name with fields is the object `{"name": "rejected", "status": 486}`.
//!
//! The alternative — an object in every case — was rejected because it makes the common case
//! (`"busy"`) three times the text and forces every app to unwrap a single-member object to read a
//! word. The alternative in the other direction — a flat string like `"rejected:486"` — was
//! rejected because it invents a second parser for something JSON already has a shape for.

use std::collections::BTreeMap;

use crate::json::Json;

/// Write a bare name.
pub(crate) fn name(value: &str) -> Json {
    Json::Str(value.to_owned())
}

/// Write a name with a `status` field — the only field any of the three carries in `v1`.
pub(crate) fn with_status(tag: &str, status: u16) -> Json {
    let mut members = BTreeMap::new();
    members.insert("name".to_owned(), Json::Str(tag.to_owned()));
    members.insert("status".to_owned(), Json::from(status));
    Json::Object(members)
}

/// Read one back: its name, and its `status` if it has one.
///
/// Returns `None` for anything that is neither a string nor an object with a string `name` — the
/// caller turns that into the contract's own error rather than this module guessing.
pub(crate) fn read(value: &Json) -> Option<(&str, Option<u16>)> {
    match value {
        Json::Str(tag) => Some((tag, None)),
        Json::Object(_) => {
            let tag = value.get("name")?.as_str()?;
            let status = match value.get("status") {
                Some(status) => Some(u16::try_from(status.as_i64()?).ok()?),
                None => None,
            };
            Some((tag, status))
        }
        _ => None,
    }
}
