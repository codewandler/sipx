//! Privacy preferences (RFC 3323 §4.2 and verified erratum 5184).
//!
//! The delimiter and construction rules live here so applications and forwarding policy consume
//! typed values rather than each growing a subtly different comma-list parser.

use std::collections::HashSet;

use bytes::Bytes;

use crate::error::HeaderError;
use crate::headers::grammar::{self, is_token_char, trim};
use crate::message::TypedHeader;
use crate::name::HeaderName;

const LABEL: &str = "Privacy";

/// One value from a `Privacy` header.
///
/// The first seven variants are the values in the IANA SIP Privacy Header Field Values registry.
/// `Extension` retains the spelling of a later token so policy can recognize it without waiting
/// for this enum to gain another variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PrivacyValue {
    /// Request user-level privacy.
    User,
    /// Request privacy for identifying routing headers.
    Header,
    /// Request privacy for session media.
    Session,
    /// Explicitly request no privacy service.
    None,
    /// Require requested privacy services to succeed or the request to fail.
    Critical,
    /// Request privacy for asserted identity (RFC 3325 §9.3).
    Id,
    /// Request privacy for History-Info (RFC 7044 §10.1).
    History,
    /// A later registered token, preserving its spelling.
    Extension(Vec<u8>),
}

impl PrivacyValue {
    /// The token spelling used for deterministic serialization.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::User => b"user",
            Self::Header => b"header",
            Self::Session => b"session",
            Self::None => b"none",
            Self::Critical => b"critical",
            Self::Id => b"id",
            Self::History => b"history",
            Self::Extension(value) => value,
        }
    }

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        if value.is_empty() || !value.iter().all(|&octet| is_token_char(octet)) {
            return Err(syntax());
        }
        Ok(known(value).unwrap_or_else(|| Self::Extension(value.to_vec())))
    }
}

/// One value from the message-wide comma-delimited `Privacy` list.
///
/// Use [`crate::message::Headers::typed_all`] to decode and validate the complete list across
/// comma-joined and repeated rows. [`PrivacyList`] is the checked construction counterpart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Privacy(PrivacyValue);

impl Privacy {
    /// Construct one syntactically valid privacy-list element.
    pub fn new(value: PrivacyValue) -> Result<Self, HeaderError> {
        validate_value(&value)?;
        Ok(Self(value))
    }

    /// The typed privacy-list element.
    #[must_use]
    pub fn value(&self) -> &PrivacyValue {
        &self.0
    }

    /// Whether this is the requested value, using case-insensitive token comparison.
    #[must_use]
    pub fn is(&self, wanted: &PrivacyValue) -> bool {
        self.0.as_bytes().eq_ignore_ascii_case(wanted.as_bytes())
    }

    /// Serialize this canonical list element.
    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        Bytes::copy_from_slice(self.0.as_bytes())
    }
}

/// A complete validated `Privacy` list for application construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyList(Vec<Privacy>);

impl PrivacyList {
    /// Construct a complete list after enforcing its message-wide invariants.
    pub fn new(values: impl IntoIterator<Item = PrivacyValue>) -> Result<Self, HeaderError> {
        let values = values
            .into_iter()
            .map(Privacy::new)
            .collect::<Result<Vec<_>, _>>()?;
        validate(values.iter().map(|privacy| &privacy.0))?;
        Ok(Self(values))
    }

    /// The requested values in construction order.
    #[must_use]
    pub fn values(&self) -> &[Privacy] {
        &self.0
    }

    /// Whether a value occurs, using case-insensitive token comparison.
    #[must_use]
    pub fn contains(&self, wanted: &PrivacyValue) -> bool {
        self.0.iter().any(|value| value.is(wanted))
    }

    /// Serialize one canonical comma-delimited header value.
    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        let mut out = Vec::new();
        for (position, value) in self.0.iter().enumerate() {
            if position != 0 {
                out.push(b',');
            }
            out.extend_from_slice(value.0.as_bytes());
        }
        Bytes::from(out)
    }
}

impl TypedHeader for Privacy {
    const NAME: HeaderName = HeaderName::Privacy;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        PrivacyValue::decode(trim(value)).map(Self)
    }

    fn decode_list(value: &[u8]) -> Result<Vec<Self>, HeaderError> {
        grammar::split_list(value, LABEL)?
            .into_iter()
            .map(Self::decode)
            .collect()
    }

    fn validate_list(values: &[&Self]) -> Result<(), HeaderError> {
        validate(values.iter().map(|privacy| &privacy.0))
    }
}

fn validate_value(value: &PrivacyValue) -> Result<(), HeaderError> {
    let token = value.as_bytes();
    if token.is_empty() || !token.iter().all(|&octet| is_token_char(octet)) {
        return Err(syntax());
    }
    if matches!(value, PrivacyValue::Extension(_)) && known(token).is_some() {
        return Err(syntax());
    }
    Ok(())
}

fn validate<'a>(values: impl IntoIterator<Item = &'a PrivacyValue>) -> Result<(), HeaderError> {
    let values = values.into_iter().collect::<Vec<_>>();
    if values.is_empty() {
        return Err(syntax());
    }

    let mut seen = HashSet::with_capacity(values.len());
    let mut none = false;
    let mut critical = None;
    for (position, value) in values.iter().enumerate() {
        validate_value(value)?;
        let token = value.as_bytes();
        if !seen.insert(token.to_ascii_lowercase()) {
            return Err(syntax());
        }
        match *value {
            PrivacyValue::None => none = true,
            PrivacyValue::Critical => critical = Some(position),
            _ => {}
        }
    }

    if none && values.len() != 1 {
        return Err(syntax());
    }
    if let Some(position) = critical
        && (position == 0 || position + 1 != values.len())
    {
        return Err(syntax());
    }
    Ok(())
}

fn known(value: &[u8]) -> Option<PrivacyValue> {
    if value.eq_ignore_ascii_case(b"user") {
        Some(PrivacyValue::User)
    } else if value.eq_ignore_ascii_case(b"header") {
        Some(PrivacyValue::Header)
    } else if value.eq_ignore_ascii_case(b"session") {
        Some(PrivacyValue::Session)
    } else if value.eq_ignore_ascii_case(b"none") {
        Some(PrivacyValue::None)
    } else if value.eq_ignore_ascii_case(b"critical") {
        Some(PrivacyValue::Critical)
    } else if value.eq_ignore_ascii_case(b"id") {
        Some(PrivacyValue::Id)
    } else if value.eq_ignore_ascii_case(b"history") {
        Some(PrivacyValue::History)
    } else {
        None
    }
}

fn syntax() -> HeaderError {
    HeaderError::Syntax { header: LABEL }
}
