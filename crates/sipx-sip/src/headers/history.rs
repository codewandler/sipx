//! Diversion history and call reasons (RFC 7044 and RFC 3326).

use bytes::Bytes;

use crate::error::HeaderError;
use crate::escape;
use crate::headers::address::Address;
use crate::headers::grammar::{self, HeaderParam, is_token_char, trim};
use crate::message::TypedHeader;
use crate::name::HeaderName;
use crate::params::Param;
use crate::uri::Uri;

const REASON: &str = "Reason";
const HISTORY_INFO: &str = "History-Info";

/// One RFC 3326 Reason value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasonValue {
    protocol: Vec<u8>,
    cause: u16,
    text: Option<Vec<u8>>,
    extensions: Vec<HeaderParam>,
}

impl ReasonValue {
    /// A SIP response code used as a reason.
    #[must_use]
    pub fn sip(cause: crate::message::StatusCode, text: Option<Vec<u8>>) -> Self {
        Self {
            protocol: b"SIP".to_vec(),
            cause: cause.code(),
            text,
            extensions: Vec::new(),
        }
    }

    /// A Q.850 cause value.
    #[must_use]
    pub fn q850(cause: u8, text: Option<Vec<u8>>) -> Self {
        Self {
            protocol: b"Q.850".to_vec(),
            cause: u16::from(cause),
            text,
            extensions: Vec::new(),
        }
    }

    /// The protocol token, preserving an extension protocol's spelling.
    #[must_use]
    pub fn protocol(&self) -> &[u8] {
        &self.protocol
    }

    /// The decimal cause.
    #[must_use]
    pub fn cause(&self) -> u16 {
        self.cause
    }

    /// The human-readable text, without quotes.
    #[must_use]
    pub fn text(&self) -> Option<&[u8]> {
        self.text.as_deref()
    }

    /// Serialize one reason value.
    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        let mut out = self.protocol.clone();
        out.extend_from_slice(b";cause=");
        out.extend_from_slice(self.cause.to_string().as_bytes());
        if let Some(text) = &self.text {
            out.extend_from_slice(b";text=\"");
            write_quoted(text, &mut out);
            out.push(b'"');
        }
        for parameter in &self.extensions {
            write_parameter(parameter, &mut out);
        }
        Bytes::from(out)
    }

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        let value = trim(value);
        let semi = value
            .iter()
            .position(|&b| b == b';')
            .ok_or(HeaderError::Syntax { header: REASON })?;
        let protocol = trim(value.get(..semi).unwrap_or(&[]));
        if protocol.is_empty() || !protocol.iter().all(|&b| is_token_char(b)) {
            return Err(HeaderError::Syntax { header: REASON });
        }
        let params = grammar::parse_params(value.get(semi..).unwrap_or(&[]), REASON)?;
        let causes: Vec<_> = params.iter().filter(|p| p.is("cause")).collect();
        let texts: Vec<_> = params.iter().filter(|p| p.is("text")).collect();
        if causes.len() != 1 || texts.len() > 1 {
            return Err(HeaderError::Syntax { header: REASON });
        }
        let cause_bytes = causes
            .first()
            .and_then(|p| p.value.as_deref())
            .ok_or(HeaderError::Syntax { header: REASON })?;
        let cause_u64 = grammar::parse_u64(cause_bytes, REASON)?;
        let cause =
            u16::try_from(cause_u64).map_err(|_| HeaderError::OutOfRange { header: REASON })?;
        if protocol.eq_ignore_ascii_case(b"SIP") && !(100..=699).contains(&cause) {
            return Err(HeaderError::OutOfRange { header: REASON });
        }
        if protocol.eq_ignore_ascii_case(b"Q.850") && cause > 127 {
            return Err(HeaderError::OutOfRange { header: REASON });
        }
        let text = texts.first().and_then(|p| p.value.clone());
        let extensions = params
            .into_iter()
            .filter(|p| !p.is("cause") && !p.is("text"))
            .collect();
        Ok(Self {
            protocol: protocol.to_vec(),
            cause,
            text,
            extensions,
        })
    }
}

/// A comma-separated Reason header value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason(pub Vec<ReasonValue>);

impl Reason {
    /// Serialize the list for a header field.
    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        join(self.0.iter().map(ReasonValue::to_bytes))
    }
}

impl From<ReasonValue> for Reason {
    fn from(value: ReasonValue) -> Self {
        Self(vec![value])
    }
}

impl TypedHeader for Reason {
    const NAME: HeaderName = HeaderName::Reason;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        grammar::split_list(value, REASON)?
            .into_iter()
            .map(ReasonValue::decode)
            .collect::<Result<Vec<_>, _>>()
            .and_then(|values| {
                (!values.is_empty())
                    .then_some(Self(values))
                    .ok_or(HeaderError::Syntax { header: REASON })
            })
    }
}

/// A hierarchical RFC 7044 history index.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HistoryIndex(Vec<u32>);

impl HistoryIndex {
    /// The mandatory first index.
    #[must_use]
    pub fn first() -> Self {
        Self(vec![1])
    }

    /// Append a component used for a forwarding action.
    #[must_use]
    pub fn forwarded(&self) -> Self {
        let mut components = self.0.clone();
        components.push(1);
        Self(components)
    }

    /// Append the visible zero which records a missing hop.
    #[must_use]
    pub fn gap(&self) -> Self {
        let mut components = self.0.clone();
        components.push(0);
        Self(components)
    }

    /// Serialize the dotted decimal form.
    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        let mut out = Vec::new();
        for (position, component) in self.0.iter().enumerate() {
            if position != 0 {
                out.push(b'.');
            }
            out.extend_from_slice(component.to_string().as_bytes());
        }
        Bytes::from(out)
    }

    fn parse(value: &[u8]) -> Result<Self, HeaderError> {
        let mut components = Vec::new();
        for component in value.split(|&b| b == b'.') {
            if component.is_empty() || (component.len() > 1 && component.first() == Some(&b'0')) {
                return Err(HeaderError::Syntax {
                    header: HISTORY_INFO,
                });
            }
            let number = grammar::parse_u64(component, HISTORY_INFO)?;
            components.push(u32::try_from(number).map_err(|_| HeaderError::OutOfRange {
                header: HISTORY_INFO,
            })?);
        }
        (!components.is_empty())
            .then_some(Self(components))
            .ok_or(HeaderError::Syntax {
                header: HISTORY_INFO,
            })
    }
}

/// Why the target represented by a History-Info entry differs from its predecessor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetChange {
    /// Request-URI change for the same target user.
    Rc(HistoryIndex),
    /// Request-URI change to a different target user.
    Mp(HistoryIndex),
    /// No Request-URI change.
    Np(HistoryIndex),
}

impl TargetChange {
    fn index(&self) -> &HistoryIndex {
        match self {
            Self::Rc(index) | Self::Mp(index) | Self::Np(index) => index,
        }
    }

    fn name(&self) -> &'static [u8] {
        match self {
            Self::Rc(_) => b"rc",
            Self::Mp(_) => b"mp",
            Self::Np(_) => b"np",
        }
    }
}

/// Target-change semantics selected when extending a history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetChangeKind {
    /// Same target user, changed Request-URI.
    Rc,
    /// Different target user.
    Mp,
    /// Unchanged Request-URI.
    Np,
}

impl TargetChangeKind {
    fn with(self, index: HistoryIndex) -> TargetChange {
        match self {
            Self::Rc => TargetChange::Rc(index),
            Self::Mp => TargetChange::Mp(index),
            Self::Np => TargetChange::Np(index),
        }
    }
}

/// One History-Info entry.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    /// The URI targeted at this hop.
    pub target: Uri,
    /// The hierarchical position of this hop.
    pub index: HistoryIndex,
    /// The preceding target and the semantics of the change.
    pub change: Option<TargetChange>,
    extensions: Vec<HeaderParam>,
}

impl HistoryEntry {
    /// Construct an entry without extensions.
    #[must_use]
    pub fn new(target: Uri, index: HistoryIndex, change: Option<TargetChange>) -> Self {
        Self {
            target,
            index,
            change,
            extensions: Vec::new(),
        }
    }

    /// Reasons embedded in this entry's targeted-to URI.
    pub fn reasons(&self) -> Result<Vec<ReasonValue>, HeaderError> {
        let Some(headers) = self.target.headers() else {
            return Ok(Vec::new());
        };
        let mut reasons = Vec::new();
        for header in headers.iter().filter(|p| p.has_name("Reason")) {
            let encoded = header
                .value()
                .ok_or(HeaderError::Syntax { header: REASON })?;
            let decoded = escape::decode(encoded).ok_or(HeaderError::Syntax { header: REASON })?;
            reasons.extend(Reason::decode(&decoded)?.0);
        }
        Ok(reasons)
    }

    fn embed_reason(&mut self, reason: &ReasonValue) {
        let encoded = percent_encode(&reason.to_bytes());
        let _ = self.target.push_header(Param::new(
            Bytes::from_static(b"Reason"),
            Bytes::from(encoded),
        ));
    }

    fn wants_privacy(&self) -> Result<bool, HeaderError> {
        let Some(headers) = self.target.headers() else {
            return Ok(false);
        };
        for header in headers.iter().filter(|p| p.has_name("Privacy")) {
            let encoded = header.value().ok_or(HeaderError::Syntax {
                header: HISTORY_INFO,
            })?;
            let decoded = escape::decode(encoded).ok_or(HeaderError::Syntax {
                header: HISTORY_INFO,
            })?;
            if decoded.eq_ignore_ascii_case(b"history") {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn anonymize(&mut self) -> Result<(), HeaderError> {
        self.target = Uri::parse(Bytes::from_static(b"sip:anonymous@anonymous.invalid")).map_err(
            |source| HeaderError::Uri {
                header: HISTORY_INFO,
                source,
            },
        )?;
        self.extensions.clear();
        Ok(())
    }

    /// Serialize one entry.
    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        let mut out = Vec::new();
        out.push(b'<');
        out.extend_from_slice(&self.target.to_bytes());
        out.extend_from_slice(b">;index=");
        out.extend_from_slice(&self.index.to_bytes());
        if let Some(change) = &self.change {
            out.push(b';');
            out.extend_from_slice(change.name());
            out.push(b'=');
            out.extend_from_slice(&change.index().to_bytes());
        }
        for parameter in &self.extensions {
            write_parameter(parameter, &mut out);
        }
        Bytes::from(out)
    }

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        let address = Address::parse(value, HISTORY_INFO)?;
        let indices: Vec<_> = address.params.iter().filter(|p| p.is("index")).collect();
        if indices.len() != 1 {
            return Err(HeaderError::Syntax {
                header: HISTORY_INFO,
            });
        }
        let index = HistoryIndex::parse(indices.first().and_then(|p| p.value.as_deref()).ok_or(
            HeaderError::Syntax {
                header: HISTORY_INFO,
            },
        )?)?;
        let changes: Vec<_> = address
            .params
            .iter()
            .filter(|p| p.is("rc") || p.is("mp") || p.is("np"))
            .collect();
        if changes.len() > 1 {
            return Err(HeaderError::Syntax {
                header: HISTORY_INFO,
            });
        }
        let change = changes
            .first()
            .map(|parameter| {
                let referenced = HistoryIndex::parse(parameter.value.as_deref().ok_or(
                    HeaderError::Syntax {
                        header: HISTORY_INFO,
                    },
                )?)?;
                Ok(if parameter.is("rc") {
                    TargetChange::Rc(referenced)
                } else if parameter.is("mp") {
                    TargetChange::Mp(referenced)
                } else {
                    TargetChange::Np(referenced)
                })
            })
            .transpose()?;
        let extensions = address
            .params
            .into_iter()
            .filter(|p| !p.is("index") && !p.is("rc") && !p.is("mp") && !p.is("np"))
            .collect();
        Ok(Self {
            target: address.uri,
            index,
            change,
            extensions,
        })
    }
}

/// A complete History-Info list in wire order.
#[derive(Debug, Clone, Default)]
pub struct HistoryInfo(pub Vec<HistoryEntry>);

impl HistoryInfo {
    /// Parse every History-Info row as one ordered cache.
    ///
    /// RFC 3261 §7.3 makes repeated list-header rows equivalent to one comma-joined row. The
    /// history indices must therefore be validated across the joined value: decoding a later row
    /// by itself would incorrectly reject its first `1.1` entry for not beginning at `1`.
    pub fn from_headers(headers: &crate::message::Headers) -> Option<Result<Self, HeaderError>> {
        let mut joined = Vec::new();
        for header in headers.get_all(&HeaderName::HistoryInfo) {
            if !joined.is_empty() {
                joined.extend_from_slice(b", ");
            }
            joined.extend_from_slice(&header.value());
        }
        (!joined.is_empty()).then(|| Self::decode(&joined))
    }

    /// Start a history at the mandatory first index.
    #[must_use]
    pub fn initial(target: Uri) -> Self {
        Self(vec![HistoryEntry::new(target, HistoryIndex::first(), None)])
    }

    /// Extend this UA's history for a retargeting action.
    ///
    /// If the received cache omitted the actual previous Request-URI, the inserted `.0` entry
    /// makes that gap visible before the new `.1` entry is appended.
    #[must_use]
    pub fn retargeted(
        mut self,
        previous: Uri,
        next: Uri,
        reason: &ReasonValue,
        kind: TargetChangeKind,
    ) -> Self {
        if self.0.is_empty() {
            self = Self::initial(previous.clone());
        }
        let last_matches = self
            .0
            .last()
            .is_some_and(|entry| entry.target.equivalent(&previous));
        if !last_matches {
            let gap_index = self
                .0
                .last()
                .map_or_else(HistoryIndex::first, |entry| entry.index.gap());
            self.0.push(HistoryEntry::new(previous, gap_index, None));
        }
        let previous_index = self
            .0
            .last()
            .map_or_else(HistoryIndex::first, |entry| entry.index.clone());
        if let Some(entry) = self.0.last_mut() {
            entry.embed_reason(reason);
        }
        self.0.push(HistoryEntry::new(
            next,
            previous_index.forwarded(),
            Some(kind.with(previous_index)),
        ));
        self
    }

    /// Apply RFC 7044 history privacy before emitting the cache.
    pub fn apply_privacy(&mut self, message_privacy: bool) -> Result<(), HeaderError> {
        for entry in &mut self.0 {
            if message_privacy || entry.wants_privacy()? {
                entry.anonymize()?;
            }
        }
        Ok(())
    }

    /// Apply message-level `Privacy: history` or `Privacy: header`, plus any entry-level
    /// privacy marker, before emitting this cache.
    pub fn apply_message_privacy(
        &mut self,
        headers: &crate::message::Headers,
    ) -> Result<(), HeaderError> {
        self.apply_privacy(message_requests_history_privacy(headers)?)
    }

    /// Serialize the complete comma-separated list.
    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        join(self.0.iter().map(HistoryEntry::to_bytes))
    }
}

/// Build the history a UAS returns in a non-100 response.
///
/// A malformed typed history is omitted: reflecting its opaque bytes could leak a target after
/// a privacy request, while response construction itself must remain infallible for a syntactically
/// valid request.
pub(crate) fn for_response(
    request: &crate::message::Request,
    status: crate::message::StatusCode,
) -> Option<Bytes> {
    if status.code() == 100 {
        return None;
    }
    let mut history = if let Some(parsed) = HistoryInfo::from_headers(&request.headers) {
        parsed.ok()?
    } else {
        let supported = request
            .headers
            .typed_all::<crate::headers::Supported>()
            .filter_map(Result::ok)
            .any(|tags| tags.contains("histinfo"));
        if !supported {
            return None;
        }
        HistoryInfo::initial(request.uri.clone())
    };
    let message_privacy = message_requests_history_privacy(&request.headers).ok()?;
    history.apply_privacy(message_privacy).ok()?;
    Some(history.to_bytes())
}

fn message_requests_history_privacy(
    headers: &crate::message::Headers,
) -> Result<bool, HeaderError> {
    let mut requested = false;
    for privacy in headers.typed_all::<crate::headers::Privacy>() {
        let privacy = privacy?;
        requested |= privacy.is(&crate::headers::PrivacyValue::History)
            || privacy.is(&crate::headers::PrivacyValue::Header);
    }
    Ok(requested)
}

impl TypedHeader for HistoryInfo {
    const NAME: HeaderName = HeaderName::HistoryInfo;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        let entries = grammar::split_list(value, HISTORY_INFO)?
            .into_iter()
            .map(HistoryEntry::decode)
            .collect::<Result<Vec<_>, _>>()?;
        if entries.is_empty() {
            return Err(HeaderError::Syntax {
                header: HISTORY_INFO,
            });
        }
        if entries
            .first()
            .is_none_or(|entry| entry.index != HistoryIndex::first())
        {
            return Err(HeaderError::Syntax {
                header: HISTORY_INFO,
            });
        }
        for (position, entry) in entries.iter().enumerate() {
            if let Some(change) = &entry.change {
                let prior = entries
                    .get(..position)
                    .unwrap_or(&[])
                    .iter()
                    .any(|candidate| candidate.index == *change.index());
                if !prior {
                    return Err(HeaderError::Syntax {
                        header: HISTORY_INFO,
                    });
                }
            }
        }
        Ok(Self(entries))
    }
}

fn join(values: impl Iterator<Item = Bytes>) -> Bytes {
    let mut out = Vec::new();
    for (position, value) in values.enumerate() {
        if position != 0 {
            out.extend_from_slice(b", ");
        }
        out.extend_from_slice(&value);
    }
    Bytes::from(out)
}

fn write_parameter(parameter: &HeaderParam, out: &mut Vec<u8>) {
    out.push(b';');
    out.extend_from_slice(&parameter.name);
    if let Some(value) = &parameter.value {
        out.push(b'=');
        if value.iter().all(|&b| is_token_char(b)) {
            out.extend_from_slice(value);
        } else {
            out.push(b'"');
            write_quoted(value, out);
            out.push(b'"');
        }
    }
}

fn write_quoted(value: &[u8], out: &mut Vec<u8>) {
    for &byte in value {
        if matches!(byte, b'\\' | b'"') {
            out.push(b'\\');
        }
        out.push(byte);
    }
}

fn percent_encode(value: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = Vec::with_capacity(value.len());
    for &byte in value {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte);
        } else {
            out.push(b'%');
            out.push(*HEX.get(usize::from(byte >> 4)).unwrap_or(&b'0'));
            out.push(*HEX.get(usize::from(byte & 0x0f)).unwrap_or(&b'0'));
        }
    }
    out
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
    use crate::message::StatusCode;

    fn uri(value: &'static [u8]) -> Uri {
        Uri::parse(Bytes::from_static(value)).unwrap()
    }

    #[test]
    fn a_retargeted_request_carries_the_previous_target_and_the_reason_it_moved() {
        let history = HistoryInfo::initial(uri(b"sip:alice@example.test")).retargeted(
            uri(b"sip:alice@example.test"),
            uri(b"sip:bob@example.test"),
            &ReasonValue::sip(StatusCode::new(302).unwrap(), None),
            TargetChangeKind::Mp,
        );
        assert_eq!(
            history.to_bytes(),
            Bytes::from_static(
                b"<sip:alice@example.test?Reason=SIP%3Bcause%3D302>;index=1, <sip:bob@example.test>;index=1.1;mp=1"
            )
        );
        assert_eq!(history.0[0].reasons().unwrap()[0].cause(), 302);
    }

    #[test]
    fn a_missing_previous_target_gets_a_visible_zero_index() {
        let history = HistoryInfo::initial(uri(b"sip:first@example.test")).retargeted(
            uri(b"sip:hidden@example.test"),
            uri(b"sip:last@example.test"),
            &ReasonValue::sip(StatusCode::new(302).unwrap(), None),
            TargetChangeKind::Mp,
        );
        assert_eq!(history.0[1].index.to_bytes(), Bytes::from_static(b"1.0"));
        assert_eq!(history.0[2].index.to_bytes(), Bytes::from_static(b"1.0.1"));
    }

    #[test]
    fn history_privacy_keeps_indices_and_hides_targets() {
        let mut history = HistoryInfo::initial(uri(b"sip:alice@example.test")).retargeted(
            uri(b"sip:alice@example.test"),
            uri(b"sip:bob@example.test"),
            &ReasonValue::sip(StatusCode::new(302).unwrap(), None),
            TargetChangeKind::Mp,
        );
        history.apply_privacy(true).unwrap();
        assert_eq!(
            history.to_bytes(),
            Bytes::from_static(
                b"<sip:anonymous@anonymous.invalid>;index=1, <sip:anonymous@anonymous.invalid>;index=1.1;mp=1"
            )
        );
    }

    #[test]
    fn typed_history_rejects_a_forward_target_reference() {
        assert!(HistoryInfo::decode(b"<sip:a@b>;index=1;mp=1.1, <sip:c@d>;index=1.1").is_err());
    }

    #[test]
    fn typed_history_requires_the_first_index_to_be_one() {
        assert!(HistoryInfo::decode(b"<sip:a@b>;index=2").is_err());
        assert!(HistoryInfo::decode(b"<sip:a@b>;index=1").is_ok());
    }

    #[test]
    fn a_tel_target_does_not_receive_a_uri_reason_component() {
        let history = HistoryInfo::initial(uri(b"tel:+12015550123")).retargeted(
            uri(b"tel:+12015550123"),
            uri(b"sip:bob@example.test"),
            &ReasonValue::sip(StatusCode::new(302).unwrap(), None),
            TargetChangeKind::Mp,
        );
        assert_eq!(
            history.0[0].target.to_bytes(),
            Bytes::from_static(b"tel:+12015550123")
        );
        assert!(history.0[0].reasons().unwrap().is_empty());
    }

    #[test]
    fn reason_validates_protocol_specific_ranges() {
        assert!(Reason::decode(b"SIP;cause=99").is_err());
        assert!(Reason::decode(b"Q.850;cause=128").is_err());
        assert_eq!(
            Reason::decode(b"SIP;cause=486;text=Busy").unwrap().0[0].cause(),
            486
        );
    }
}
