//! SIP authenticated identity and `PASSporT` (RFC 8224 and RFC 8225).
//!
//! This module is deliberately sans-I/O: callers supply time and keys. Credential retrieval,
//! trust policy, and authorization belong to the user-agent layer.

use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use p256::ecdsa::signature::{Signer as _, Verifier as _};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::pkcs8::{DecodePrivateKey as _, DecodePublicKey as _};
use serde_json::Value;
use thiserror::Error;

use crate::error::HeaderError;
use crate::headers::grammar::{is_token_char, trim};
use crate::headers::{Date, From, To};
use crate::{HeaderName, Request, Scheme, TypedHeader, Uri};

const IDENTITY: &str = "Identity";
const ES256: &str = "ES256";

/// A parsed RFC 8224 `Identity` header value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityHeader {
    /// Full `header.payload.signature` or compact `..signature` `PASSporT`.
    pub digest: String,
    /// Credential-reference URI from `info`.
    pub info: String,
    /// JWS algorithm. RFC 8224 defaults an absent parameter to `ES256`.
    pub algorithm: String,
    /// Mandatory `PASSporT` extension, if one was requested.
    pub passport_type: Option<String>,
    /// Unknown generic parameters retained in wire order.
    pub extensions: Vec<(String, Option<String>)>,
}

impl IdentityHeader {
    /// Serialize the typed header value.
    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        let mut out = format!("{};info=<{}>", self.digest, self.info);
        if self.algorithm != ES256 {
            let _ = write!(out, ";alg={}", self.algorithm);
        }
        if let Some(passport_type) = &self.passport_type {
            let _ = write!(out, ";ppt={passport_type}");
        }
        for (name, value) in &self.extensions {
            out.push(';');
            out.push_str(name);
            if let Some(value) = value {
                out.push('=');
                out.push_str(value);
            }
        }
        Bytes::from(out)
    }
}

impl TypedHeader for IdentityHeader {
    const NAME: HeaderName = HeaderName::Identity;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        parse_identity(value).ok_or(HeaderError::Syntax { header: IDENTITY })
    }
}

fn parse_identity(value: &[u8]) -> Option<IdentityHeader> {
    let text = std::str::from_utf8(trim(value)).ok()?;
    let parts = split_identity_parts(text)?;
    let digest = parts.first()?.trim();
    if !valid_digest(digest) {
        return None;
    }

    let mut info = None;
    let mut algorithm = None;
    let mut passport_type = None;
    let mut extensions = Vec::new();
    for (index, raw) in parts.iter().skip(1).enumerate() {
        let raw = raw.trim();
        let (name, value) = raw
            .split_once('=')
            .map_or((raw, None), |(n, v)| (n, Some(v)));
        if name.is_empty() || !name.as_bytes().iter().all(|&b| is_token_char(b)) {
            return None;
        }
        match name.to_ascii_lowercase().as_str() {
            "info" => {
                if info.is_some() {
                    return None;
                }
                let value = value?;
                let uri = value.strip_prefix('<')?.strip_suffix('>')?;
                Uri::parse(Bytes::copy_from_slice(uri.as_bytes())).ok()?;
                info = Some(uri.to_owned());
            }
            "alg" => {
                if index == 0 {
                    return None;
                }
                if algorithm.is_some() {
                    return None;
                }
                let value = value?;
                if value.is_empty() || !value.as_bytes().iter().all(|&b| is_token_char(b)) {
                    return None;
                }
                algorithm = Some(value.to_owned());
            }
            "ppt" => {
                if index == 0 {
                    return None;
                }
                if passport_type.is_some() {
                    return None;
                }
                let value = value?;
                if value.is_empty() || !value.as_bytes().iter().all(|&b| is_token_char(b)) {
                    return None;
                }
                passport_type = Some(value.to_owned());
            }
            _ => {
                if index == 0 {
                    return None;
                }
                extensions.push((name.to_owned(), value.map(str::to_owned)));
            }
        }
    }
    Some(IdentityHeader {
        digest: digest.to_owned(),
        info: info?,
        algorithm: algorithm.unwrap_or_else(|| ES256.to_owned()),
        passport_type,
        extensions,
    })
}

fn split_identity_parts(value: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut in_angle = false;
    for (at, byte) in value.bytes().enumerate() {
        match byte {
            b'<' if !in_angle => in_angle = true,
            b'>' if in_angle => in_angle = false,
            b';' if !in_angle => {
                parts.push(value.get(start..at)?);
                start = at.saturating_add(1);
            }
            _ => {}
        }
    }
    if in_angle {
        return None;
    }
    parts.push(value.get(start..)?);
    Some(parts)
}

fn valid_digest(value: &str) -> bool {
    let segments: Vec<_> = value.split('.').collect();
    if segments.len() != 3 {
        return false;
    }
    let full = segments.first().is_some_and(|part| !part.is_empty())
        && segments.get(1).is_some_and(|part| !part.is_empty());
    let compact = segments.first().is_some_and(|part| part.is_empty())
        && segments.get(1).is_some_and(|part| part.is_empty());
    (full || compact)
        && segments.get(2).is_some_and(|part| !part.is_empty())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'+' | b'/' | b'.'))
}

/// A canonical origin or destination identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CanonicalIdentity {
    /// Telephone-number identity, containing only digits, `*`, and `#`.
    TelephoneNumber(String),
    /// Canonical SIP/SIPS address of record.
    Uri(String),
}

impl CanonicalIdentity {
    /// Derive the RFC 8224 §§8.1, 8.3, and 8.5 identity from a URI.
    pub fn from_uri(uri: &Uri) -> Result<Self, IdentityError> {
        match uri.scheme() {
            Scheme::Tel => canonical_number(uri.opaque().unwrap_or_default()),
            Scheme::Sip | Scheme::Sips => {
                let telephone = uri
                    .params()
                    .and_then(|params| params.value("user"))
                    .is_some_and(|value| value.eq_ignore_ascii_case(b"phone"));
                if telephone {
                    return canonical_number(&uri.decoded_user().ok_or(IdentityError::Identity)?);
                }
                let user = uri.user().ok_or(IdentityError::Identity)?;
                let user = normalized_user(user)?;
                let host = uri.host().ok_or(IdentityError::Identity)?.to_string();
                let scheme = if matches!(uri.scheme(), Scheme::Sips) {
                    "sips"
                } else {
                    "sip"
                };
                Ok(Self::Uri(format!(
                    "{scheme}:{}@{}",
                    user.to_ascii_lowercase(),
                    host.to_ascii_lowercase()
                )))
            }
            Scheme::Other(_) => Err(IdentityError::Identity),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::TelephoneNumber(_) => "tn",
            Self::Uri(_) => "uri",
        }
    }

    fn value(&self) -> &str {
        match self {
            Self::TelephoneNumber(value) | Self::Uri(value) => value,
        }
    }
}

fn canonical_number(raw: &[u8]) -> Result<CanonicalIdentity, IdentityError> {
    let before_params = raw.split(|&b| b == b';').next().unwrap_or_default();
    let number: String = before_params
        .iter()
        .copied()
        .filter(|b| b.is_ascii_digit() || matches!(b, b'*' | b'#'))
        .map(char::from)
        .collect();
    if number.is_empty() {
        Err(IdentityError::Identity)
    } else {
        Ok(CanonicalIdentity::TelephoneNumber(number))
    }
}

fn normalized_user(raw: &[u8]) -> Result<String, IdentityError> {
    let mut out = String::new();
    let mut at = 0usize;
    while let Some(&byte) = raw.get(at) {
        if byte == b'%' {
            let hi = raw.get(at.saturating_add(1)).copied().and_then(hex_value);
            let lo = raw.get(at.saturating_add(2)).copied().and_then(hex_value);
            let decoded = hi
                .zip(lo)
                .map(|(hi, lo)| hi * 16 + lo)
                .ok_or(IdentityError::Identity)?;
            if decoded.is_ascii_alphanumeric() || matches!(decoded, b'-' | b'.' | b'_' | b'~') {
                out.push(char::from(decoded));
            } else {
                let _ = write!(out, "%{decoded:02X}");
            }
            at = at.saturating_add(3);
        } else {
            if !byte.is_ascii() {
                return Err(IdentityError::Identity);
            }
            out.push(char::from(byte));
            at = at.saturating_add(1);
        }
    }
    if out.is_empty() {
        Err(IdentityError::Identity)
    } else {
        Ok(out)
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// An `ES256` private key. Its debug form never includes key material.
pub struct Es256SigningKey(SigningKey);

impl std::fmt::Debug for Es256SigningKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Es256SigningKey")
            .finish_non_exhaustive()
    }
}

impl Es256SigningKey {
    /// Read an unencrypted `PKCS #8` PEM private key.
    pub fn from_pkcs8_pem(pem: &str) -> Result<Self, IdentityError> {
        SigningKey::from_pkcs8_pem(pem)
            .map(Self)
            .map_err(|_| IdentityError::Credential)
    }

    /// The public key corresponding to this private key.
    #[must_use]
    pub fn verifying_key(&self) -> Es256VerifyingKey {
        Es256VerifyingKey(*self.0.verifying_key())
    }
}

/// An `ES256` public key.
#[derive(Clone)]
pub struct Es256VerifyingKey(VerifyingKey);

impl std::fmt::Debug for Es256VerifyingKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Es256VerifyingKey")
            .finish_non_exhaustive()
    }
}

impl Es256VerifyingKey {
    /// Read a `SubjectPublicKeyInfo` PEM public key.
    pub fn from_public_key_pem(pem: &str) -> Result<Self, IdentityError> {
        VerifyingKey::from_public_key_pem(pem)
            .map(Self)
            .map_err(|_| IdentityError::Credential)
    }
}

/// A `PASSporT` construction or verification failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum IdentityError {
    /// A SIP identity could not be derived or canonicalized.
    #[error("invalid SIP identity")]
    Identity,
    /// The `PASSporT` grammar, base64url, JSON, or required baseline claims are invalid.
    #[error("invalid PASSporT")]
    Passport,
    /// The algorithm is not mandatory-to-implement ES256.
    #[error("unsupported PASSporT algorithm")]
    Algorithm,
    /// A mandatory `PASSporT` extension is not supported.
    #[error("unsupported PASSporT type")]
    PassportType,
    /// ES256 key material is not usable.
    #[error("unsupported ES256 credential")]
    Credential,
    /// The ES256 signature did not verify.
    #[error("invalid PASSporT signature")]
    Signature,
    /// A SIP Date cannot be represented or decoded.
    #[error("invalid SIP Date")]
    Date,
}

/// Canonical From and To identities from one SIP request.
pub fn request_identities(
    request: &Request,
) -> Result<(CanonicalIdentity, CanonicalIdentity), IdentityError> {
    let from = request
        .headers
        .typed::<From>()
        .ok_or(IdentityError::Identity)?
        .map_err(|_| IdentityError::Identity)?;
    let to = request
        .headers
        .typed::<To>()
        .ok_or(IdentityError::Identity)?
        .map_err(|_| IdentityError::Identity)?;
    Ok((
        CanonicalIdentity::from_uri(&from.uri)?,
        CanonicalIdentity::from_uri(&to.uri)?,
    ))
}

/// Construct and deterministically sign a full baseline `PASSporT`.
pub fn sign_passport(
    key: &Es256SigningKey,
    origin: &CanonicalIdentity,
    destination: &CanonicalIdentity,
    issued_at: i64,
    info: &str,
) -> IdentityHeader {
    let protected = protected_json(info);
    let payload = payload_json(origin, destination, issued_at);
    let protected = URL_SAFE_NO_PAD.encode(protected.as_bytes());
    let payload = URL_SAFE_NO_PAD.encode(payload.as_bytes());
    let signing_input = format!("{protected}.{payload}");
    let signature: Signature = key.0.sign(signing_input.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(signature.to_bytes());
    IdentityHeader {
        digest: format!("{signing_input}.{signature}"),
        info: info.to_owned(),
        algorithm: ES256.to_owned(),
        passport_type: None,
        extensions: Vec::new(),
    }
}

/// Verify a full or compact baseline `PASSporT` against SIP-derived claims.
pub fn verify_passport(
    header: &IdentityHeader,
    key: &Es256VerifyingKey,
    origin: &CanonicalIdentity,
    destination: &CanonicalIdentity,
    issued_at: i64,
) -> Result<(), IdentityError> {
    if header.passport_type.is_some() {
        return Err(IdentityError::PassportType);
    }
    if header.algorithm != ES256 {
        return Err(IdentityError::Algorithm);
    }
    let mut parts = header.digest.split('.');
    let encoded_header = parts.next().ok_or(IdentityError::Passport)?;
    let encoded_payload = parts.next().ok_or(IdentityError::Passport)?;
    let encoded_signature = parts.next().ok_or(IdentityError::Passport)?;
    if parts.next().is_some() || encoded_signature.is_empty() {
        return Err(IdentityError::Passport);
    }

    let expected_header = protected_json(&header.info);
    let expected_payload = payload_json(origin, destination, issued_at);
    let (protected, payload) = if encoded_header.is_empty() && encoded_payload.is_empty() {
        (
            URL_SAFE_NO_PAD.encode(expected_header.as_bytes()),
            URL_SAFE_NO_PAD.encode(expected_payload.as_bytes()),
        )
    } else if !encoded_header.is_empty() && !encoded_payload.is_empty() {
        validate_full_json(
            encoded_header,
            encoded_payload,
            &expected_header,
            &expected_payload,
        )?;
        (encoded_header.to_owned(), encoded_payload.to_owned())
    } else {
        return Err(IdentityError::Passport);
    };

    let signature = URL_SAFE_NO_PAD
        .decode(encoded_signature)
        .map_err(|_| IdentityError::Passport)?;
    let signature = Signature::from_slice(&signature).map_err(|_| IdentityError::Passport)?;
    let signing_input = format!("{protected}.{payload}");
    verify_es256(&key.0, signing_input.as_bytes(), &signature)
}

/// Read `iat` from a full `PASSporT`; a compact form carries none to read.
pub fn passport_issued_at(header: &IdentityHeader) -> Result<Option<i64>, IdentityError> {
    let mut parts = header.digest.split('.');
    let encoded_header = parts.next().ok_or(IdentityError::Passport)?;
    let encoded_payload = parts.next().ok_or(IdentityError::Passport)?;
    let signature = parts.next().ok_or(IdentityError::Passport)?;
    if parts.next().is_some() || signature.is_empty() {
        return Err(IdentityError::Passport);
    }
    if encoded_header.is_empty() && encoded_payload.is_empty() {
        return Ok(None);
    }
    if encoded_header.is_empty() || encoded_payload.is_empty() {
        return Err(IdentityError::Passport);
    }
    let payload = URL_SAFE_NO_PAD
        .decode(encoded_payload)
        .map_err(|_| IdentityError::Passport)?;
    let value: Value = serde_json::from_slice(&payload).map_err(|_| IdentityError::Passport)?;
    value
        .as_object()
        .and_then(|object| object.get("iat"))
        .and_then(Value::as_i64)
        .map(Some)
        .ok_or(IdentityError::Passport)
}

fn verify_es256(
    key: &VerifyingKey,
    signing_input: &[u8],
    signature: &Signature,
) -> Result<(), IdentityError> {
    if key.verify(signing_input, signature).is_ok() {
        return Ok(());
    }
    // JWS specifies the mathematical ECDSA signature and does not require low-S normalization.
    // Some cryptographic backends enforce low-S to avoid malleability, so accept the equivalent
    // normalized form when the wire carried high-S.
    signature
        .normalize_s()
        .ok_or(IdentityError::Signature)
        .and_then(|normalized| {
            key.verify(signing_input, &normalized)
                .map_err(|_| IdentityError::Signature)
        })
}

fn validate_full_json(
    encoded_header: &str,
    encoded_payload: &str,
    expected_header: &str,
    expected_payload: &str,
) -> Result<(), IdentityError> {
    let header = URL_SAFE_NO_PAD
        .decode(encoded_header)
        .map_err(|_| IdentityError::Passport)?;
    let payload = URL_SAFE_NO_PAD
        .decode(encoded_payload)
        .map_err(|_| IdentityError::Passport)?;
    let header_value: Value =
        serde_json::from_slice(&header).map_err(|_| IdentityError::Passport)?;
    let payload_value: Value =
        serde_json::from_slice(&payload).map_err(|_| IdentityError::Passport)?;
    let expected_header_value: Value =
        serde_json::from_str(expected_header).map_err(|_| IdentityError::Passport)?;
    let expected_payload_value: Value =
        serde_json::from_str(expected_payload).map_err(|_| IdentityError::Passport)?;
    if header_value != expected_header_value || payload_value != expected_payload_value {
        return Err(IdentityError::Passport);
    }
    // Semantic equality is not enough: RFC 8225 §9 makes the deterministic octets normative.
    if header.as_slice() != expected_header.as_bytes()
        || payload.as_slice() != expected_payload.as_bytes()
    {
        return Err(IdentityError::Passport);
    }
    Ok(())
}

fn protected_json(info: &str) -> String {
    let info = serde_json::to_string(info).unwrap_or_else(|_| "\"\"".to_owned());
    format!(r#"{{"alg":"ES256","typ":"passport","x5u":{info}}}"#)
}

fn payload_json(
    origin: &CanonicalIdentity,
    destination: &CanonicalIdentity,
    issued_at: i64,
) -> String {
    let origin_value = serde_json::to_string(origin.value()).unwrap_or_else(|_| "\"\"".to_owned());
    let destination_value =
        serde_json::to_string(destination.value()).unwrap_or_else(|_| "\"\"".to_owned());
    format!(
        r#"{{"dest":{{"{}":[{}]}},"iat":{},"orig":{{"{}":{}}}}}"#,
        destination.kind(),
        destination_value,
        issued_at,
        origin.kind(),
        origin_value
    )
}

/// Parse one already syntax-checked SIP Date into Unix seconds.
pub fn date_timestamp(date: &Date) -> Result<i64, IdentityError> {
    let value = date.0.as_slice();
    if value.len() != 29 {
        return Err(IdentityError::Date);
    }
    let number = |from: usize, to: usize| -> Result<i64, IdentityError> {
        let bytes = value.get(from..to).ok_or(IdentityError::Date)?;
        let text = std::str::from_utf8(bytes).map_err(|_| IdentityError::Date)?;
        text.parse().map_err(|_| IdentityError::Date)
    };
    let year = number(12, 16)?;
    let month = month_number(value.get(8..11).ok_or(IdentityError::Date)?)?;
    let day = number(5, 7)?;
    let hour = number(17, 19)?;
    let minute = number(20, 22)?;
    let second = number(23, 25)?;
    if !(1..=9999).contains(&year)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(IdentityError::Date);
    }
    let days =
        days_before_year(year) + days_before_month(year, month) + day - 1 - days_before_year(1970);
    let weekday = weekday_name(days);
    if value.get(0..3) != Some(weekday.as_bytes()) {
        return Err(IdentityError::Date);
    }
    Ok(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Format Unix seconds as the one GMT date form SIP permits.
pub fn date_from_timestamp(timestamp: i64) -> Result<Date, IdentityError> {
    const MIN: i64 = -62_135_596_800; // 0001-01-01 00:00:00 UTC
    const MAX: i64 = 253_402_300_799; // 9999-12-31 23:59:59 UTC
    if !(MIN..=MAX).contains(&timestamp) {
        return Err(IdentityError::Date);
    }
    let days = timestamp.div_euclid(86_400);
    let seconds = timestamp.rem_euclid(86_400);
    let mut year = 1970i64;
    let mut day_of_year = days;
    if day_of_year >= 0 {
        while day_of_year >= days_in_year(year) {
            day_of_year -= days_in_year(year);
            year += 1;
        }
    } else {
        while day_of_year < 0 {
            year -= 1;
            day_of_year += days_in_year(year);
        }
    }
    let mut month = 1i64;
    while day_of_year >= days_in_month(year, month) {
        day_of_year -= days_in_month(year, month);
        month += 1;
    }
    let day = day_of_year + 1;
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    let value = format!(
        "{}, {day:02} {} {year:04} {hour:02}:{minute:02}:{second:02} GMT",
        weekday_name(days),
        month_name(month)?
    );
    Ok(Date(value.into_bytes()))
}

fn month_number(month: &[u8]) -> Result<i64, IdentityError> {
    const MONTHS: [&[u8]; 12] = [
        b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov",
        b"Dec",
    ];
    MONTHS
        .iter()
        .position(|candidate| *candidate == month)
        .and_then(|index| i64::try_from(index).ok())
        .map(|index| index + 1)
        .ok_or(IdentityError::Date)
}

fn month_name(month: i64) -> Result<&'static str, IdentityError> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    usize::try_from(month - 1)
        .ok()
        .and_then(|index| MONTHS.get(index).copied())
        .ok_or(IdentityError::Date)
}

fn weekday_name(days_since_epoch: i64) -> &'static str {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    usize::try_from((days_since_epoch + 3).rem_euclid(7))
        .ok()
        .and_then(|index| DAYS.get(index).copied())
        .unwrap_or("Thu")
}

fn leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_year(year: i64) -> i64 {
    if leap(year) { 366 } else { 365 }
}

fn days_before_year(year: i64) -> i64 {
    let previous = year - 1;
    previous * 365 + previous / 4 - previous / 100 + previous / 400
}

fn days_before_month(year: i64, month: i64) -> i64 {
    (1..month)
        .map(|candidate| days_in_month(year, candidate))
        .sum()
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap(year) => 29,
        2 => 28,
        _ => 0,
    }
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

    const PRIVATE: &str = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgi7q2TZvN9VDFg8Vy\n\
qCP06bETrR2v8MRvr89rn4i+UAahRANCAAQWfaj1HUETpoNCrOtp9KA8o0V79IuW\n\
ARKt9C1cFPkyd3FBP4SeiNZxQhDrD0tdBHls3/wFe8++K2FrPyQF9vuh\n\
-----END PRIVATE KEY-----";
    const PUBLIC: &str = "-----BEGIN PUBLIC KEY-----\n\
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAE8HNbQd/TmvCKwPKHkMF9fScavGeH\n\
78YTU8qLS8I5HLHSSmlATLcslQMhNC/OhlWBYC626nIlo7XeebYS7Sb37g==\n\
-----END PUBLIC KEY-----";
    const RFC_TOKEN: &str = concat!(
        "eyJhbGciOiJFUzI1NiIsInR5cCI6InBhc3Nwb3J0IiwieDV1IjoiaHR0cHM6Ly9j",
        "ZXJ0LmV4YW1wbGUub3JnL3Bhc3Nwb3J0LmNlciJ9.",
        "eyJkZXN0Ijp7InVyaSI6WyJzaXA6YWxpY2VAZXhhbXBsZS5jb20iXX0sImlhdCI",
        "6MTQ3MTM3NTQxOCwib3JpZyI6eyJ0biI6IjEyMTU1NTUxMjEyIn19.",
        "VLBCIVDCaeK6M4hLJb6SHQvacAQVvoiiEOWQ_iUkqk79UD81fHQ0E1b3_GluIkb",
        "a7UWYRM47ZbNFdOJquE35cw"
    );

    #[test]
    fn rfc8225_appendix_a_is_the_signing_oracle() {
        let key = Es256SigningKey::from_pkcs8_pem(PRIVATE).unwrap();
        let origin = CanonicalIdentity::TelephoneNumber("12155551212".to_owned());
        let destination = CanonicalIdentity::Uri("sip:alice@example.com".to_owned());
        let identity = sign_passport(
            &key,
            &origin,
            &destination,
            1_471_375_418,
            "https://cert.example.org/passport.cer",
        );
        let repeated = sign_passport(
            &key,
            &origin,
            &destination,
            1_471_375_418,
            "https://cert.example.org/passport.cer",
        );
        assert_eq!(identity.digest, repeated.digest);
        let expected_signing_input = RFC_TOKEN.rsplit_once('.').unwrap().0;
        assert_eq!(
            identity.digest.rsplit_once('.').unwrap().0,
            expected_signing_input
        );
        verify_passport(
            &identity,
            &key.verifying_key(),
            &origin,
            &destination,
            1_471_375_418,
        )
        .unwrap();
    }

    #[test]
    fn a_valid_signature_over_differently_ordered_json_is_rejected() {
        let key = Es256SigningKey::from_pkcs8_pem(PRIVATE).unwrap();
        let origin = CanonicalIdentity::TelephoneNumber("12155551212".to_owned());
        let destination = CanonicalIdentity::Uri("sip:alice@example.com".to_owned());
        let mut identity = sign_passport(
            &key,
            &origin,
            &destination,
            1_471_375_418,
            "https://cert.example.org/passport.cer",
        );
        let mut parts = identity.digest.split('.');
        let _original_header = parts.next().unwrap();
        let payload = parts.next().unwrap();
        let _original_signature = parts.next().unwrap();
        let reordered_header = URL_SAFE_NO_PAD.encode(
            br#"{"typ":"passport","alg":"ES256","x5u":"https://cert.example.org/passport.cer"}"#,
        );
        let signing_input = format!("{reordered_header}.{payload}");
        let signature: Signature = key.0.sign(signing_input.as_bytes());
        identity.digest = format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        );
        assert_eq!(
            verify_passport(
                &identity,
                &key.verifying_key(),
                &origin,
                &destination,
                1_471_375_418,
            ),
            Err(IdentityError::Passport)
        );
    }

    #[test]
    fn rfc7515_appendix_a3_is_the_es256_verification_oracle() {
        let x = URL_SAFE_NO_PAD
            .decode("f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU")
            .unwrap();
        let y = URL_SAFE_NO_PAD
            .decode("x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0")
            .unwrap();
        let mut point = Vec::with_capacity(65);
        point.push(4);
        point.extend_from_slice(&x);
        point.extend_from_slice(&y);
        let key = VerifyingKey::from_sec1_bytes(&point).unwrap();
        let signature = URL_SAFE_NO_PAD
            .decode(concat!(
                "DtEhU3ljbEg8L38VWAfUAqOyKAM6-Xx-F4GawxaepmXFCgfTjDxw5djxLa8ISlSA",
                "pmWQxfKTUJqPP3-Kg6NU1Q"
            ))
            .unwrap();
        let signature = Signature::from_slice(&signature).unwrap();
        let input = concat!(
            "eyJhbGciOiJFUzI1NiJ9.",
            "eyJpc3MiOiJqb2UiLA0KICJleHAiOjEzMDA4MTkzODAsDQogImh0dHA6Ly9leGFt",
            "cGxlLmNvbS9pc19yb290Ijp0cnVlfQ"
        );
        verify_es256(&key, input.as_bytes(), &signature).unwrap();
    }

    #[test]
    fn identity_defaults_an_absent_algorithm_to_es256() {
        let parsed = IdentityHeader::decode(
            format!("{RFC_TOKEN};info=<https://cert.example.org/passport.cer>").as_bytes(),
        )
        .unwrap();
        assert_eq!(parsed.algorithm, "ES256");
    }

    #[test]
    fn unknown_ppt_is_retained_for_mandatory_refusal() {
        let parsed = IdentityHeader::decode(
            format!("{RFC_TOKEN};info=<https://cert.example.org/c>;ppt=unknown").as_bytes(),
        )
        .unwrap();
        assert_eq!(parsed.passport_type.as_deref(), Some("unknown"));
    }

    #[test]
    fn canonicalization_distinguishes_phone_from_uri() {
        let phone = Uri::parse(Bytes::from_static(
            b"sip:+1-(215)-555-1212@example.com;user=phone",
        ))
        .unwrap();
        assert_eq!(
            CanonicalIdentity::from_uri(&phone).unwrap(),
            CanonicalIdentity::TelephoneNumber("12155551212".to_owned())
        );
        let uri = Uri::parse(Bytes::from_static(
            b"SIP:Al%69ce:secret@EXAMPLE.com:5060;transport=tcp?Subject=x",
        ))
        .unwrap();
        assert_eq!(
            CanonicalIdentity::from_uri(&uri).unwrap(),
            CanonicalIdentity::Uri("sip:alice@example.com".to_owned())
        );
    }

    #[test]
    fn sip_dates_round_trip_and_reject_a_false_weekday() {
        let date = date_from_timestamp(1_471_375_418).unwrap();
        assert_eq!(date.0, b"Tue, 16 Aug 2016 19:23:38 GMT");
        assert_eq!(date_timestamp(&date).unwrap(), 1_471_375_418);
        assert!(date_timestamp(&Date(b"Mon, 16 Aug 2016 19:23:38 GMT".to_vec())).is_err());
    }

    #[test]
    fn key_debug_reports_never_contain_key_material() {
        let private = Es256SigningKey::from_pkcs8_pem(PRIVATE).unwrap();
        let public = Es256VerifyingKey::from_public_key_pem(PUBLIC).unwrap();
        assert!(!format!("{private:?}").contains("MIGH"));
        assert!(!format!("{public:?}").contains("MFkw"));
    }
}
