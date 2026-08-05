//! The message model: requests, responses and their header collection.
//!
//! A parsed message borrows the bytes it arrived in. Header entries hold spans into that
//! buffer, so an unmodified message is written back byte for byte — including original
//! capitalization, compact forms, the whitespace around each `:`, and line folding.
//!
//! That is not fastidiousness. A proxy forwards far more headers than it inspects, and a
//! stack that normalizes whitespace on the way through breaks signature-bearing headers and
//! makes every packet capture an exercise in doubt.

use std::borrow::Cow;
use std::ops::Range;

use bytes::Bytes;

use crate::error::{AddressEditError, HeaderError, WarningEditError};
use crate::headers::address::{AddressValueSpan, value_spans};
use crate::headers::grammar::is_token_char;
use crate::headers::warning::{WarningValueSpan, value_spans as warning_value_spans};
use crate::name::HeaderName;
use crate::uri::Uri;

/// A request method.
///
/// Comparison is **case-sensitive** (RFC 3261 §7.1): `Invite` is not `INVITE`. Method tokens
/// may contain any token character, including the ones that look like punctuation — RFC 4475
/// §3.1.1.2 sends a method built from exclamation marks, percent signs, backticks and
/// apostrophes, and it is a perfectly legal method.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Method {
    /// `INVITE`
    Invite,
    /// `ACK`
    Ack,
    /// `BYE`
    Bye,
    /// `CANCEL`
    Cancel,
    /// `REGISTER`
    Register,
    /// `OPTIONS`
    Options,
    /// `INFO`
    Info,
    /// `PRACK`
    Prack,
    /// `UPDATE`
    Update,
    /// `SUBSCRIBE`
    Subscribe,
    /// `NOTIFY`
    Notify,
    /// `REFER`
    Refer,
    /// `MESSAGE`
    Message,
    /// `PUBLISH`
    Publish,
    /// Any other method token, retained verbatim.
    Other(Bytes),
}

impl Method {
    /// Resolve a method token. Never fails: an unknown method is a method.
    #[must_use]
    pub fn parse(raw: &Bytes) -> Self {
        match raw.as_ref() {
            b"INVITE" => Self::Invite,
            b"ACK" => Self::Ack,
            b"BYE" => Self::Bye,
            b"CANCEL" => Self::Cancel,
            b"REGISTER" => Self::Register,
            b"OPTIONS" => Self::Options,
            b"INFO" => Self::Info,
            b"PRACK" => Self::Prack,
            b"UPDATE" => Self::Update,
            b"SUBSCRIBE" => Self::Subscribe,
            b"NOTIFY" => Self::Notify,
            b"REFER" => Self::Refer,
            b"MESSAGE" => Self::Message,
            b"PUBLISH" => Self::Publish,
            _ => Self::Other(raw.clone()),
        }
    }

    /// The method token.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Invite => b"INVITE",
            Self::Ack => b"ACK",
            Self::Bye => b"BYE",
            Self::Cancel => b"CANCEL",
            Self::Register => b"REGISTER",
            Self::Options => b"OPTIONS",
            Self::Info => b"INFO",
            Self::Prack => b"PRACK",
            Self::Update => b"UPDATE",
            Self::Subscribe => b"SUBSCRIBE",
            Self::Notify => b"NOTIFY",
            Self::Refer => b"REFER",
            Self::Message => b"MESSAGE",
            Self::Publish => b"PUBLISH",
            Self::Other(raw) => raw,
        }
    }
}

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(self.as_bytes()))
    }
}

/// The protocol version on a start line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Version {
    /// `SIP/2.0`, the only version sipx speaks.
    Sip20,
    /// Any other version. Parsed rather than rejected so the caller can answer 505 rather
    /// than dropping the message (RFC 4475 §3.1.2.16).
    Other(Bytes),
}

impl Version {
    #[must_use]
    pub(crate) fn parse(raw: &Bytes) -> Self {
        // RFC 3261 §7.1: "The SIP-Version string is case-insensitive, but implementations
        // MUST send upper-case." Serialization stays upper-case; only recognition folds.
        if raw.eq_ignore_ascii_case(b"SIP/2.0") {
            Self::Sip20
        } else {
            Self::Other(raw.clone())
        }
    }

    /// The version token.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Sip20 => b"SIP/2.0",
            Self::Other(raw) => raw,
        }
    }

    /// Whether this is a version sipx can act on.
    #[must_use]
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Sip20)
    }
}

/// A response status code, always in `100..=699`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StatusCode(u16);

impl StatusCode {
    /// Build a status code, rejecting anything outside `100..=699`.
    #[must_use]
    pub fn new(code: u16) -> Option<Self> {
        (100..=699).contains(&code).then_some(Self(code))
    }

    /// The numeric code.
    #[must_use]
    pub fn code(self) -> u16 {
        self.0
    }

    /// The response class: 1 for provisional, 2 for success, and so on.
    #[must_use]
    pub fn class(self) -> u16 {
        self.0 / 100
    }

    /// Whether this is a provisional (1xx) response.
    #[must_use]
    pub fn is_provisional(self) -> bool {
        self.class() == 1
    }

    /// Whether this is a final (2xx and above) response.
    #[must_use]
    pub fn is_final(self) -> bool {
        !self.is_provisional()
    }

    /// Whether this is a success (2xx) response.
    #[must_use]
    pub fn is_success(self) -> bool {
        self.class() == 2
    }
}

impl std::fmt::Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One header field.
#[derive(Debug, Clone)]
pub struct Header {
    name: HeaderName,
    repr: HeaderRepr,
}

#[derive(Debug, Clone)]
enum HeaderRepr {
    /// Parsed from the wire. `line` is the field exactly as it appeared — name, whatever
    /// whitespace surrounded the colon, and the value including any folding — but not the
    /// terminating CRLF. `value_offset` indexes into it.
    Wire { line: Bytes, value_offset: usize },
    /// Constructed by this process.
    Built { value: Bytes },
}

#[derive(Debug)]
struct AddressLayout {
    spans: Vec<AddressValueSpan>,
    source_map: Vec<Range<usize>>,
    raw_len: usize,
}

#[derive(Debug)]
struct WarningLayout {
    spans: Vec<WarningValueSpan>,
    source_map: Vec<Range<usize>>,
    raw_len: usize,
}

impl Header {
    /// Build a header without checking the value.
    ///
    /// Crate-private on purpose: the only callers are the parser, which works on bytes that
    /// were already framed, and the builders in [`crate::build`], which check first. The
    /// public way to make a header is `Header::build`, and it is fallible.
    #[must_use]
    pub(crate) fn new_unchecked(name: HeaderName, value: impl Into<Bytes>) -> Self {
        Self {
            name,
            repr: HeaderRepr::Built {
                value: value.into(),
            },
        }
    }

    pub(crate) fn from_wire(name: HeaderName, line: Bytes, value_offset: usize) -> Self {
        Self {
            name,
            repr: HeaderRepr::Wire { line, value_offset },
        }
    }

    /// The resolved header name.
    #[must_use]
    pub fn name(&self) -> &HeaderName {
        &self.name
    }

    /// The value exactly as it appeared, folding included.
    #[must_use]
    pub fn raw_value(&self) -> &[u8] {
        match &self.repr {
            HeaderRepr::Wire { line, value_offset } => line.get(*value_offset..).unwrap_or(&[]),
            HeaderRepr::Built { value } => value,
        }
    }

    /// The value with line folding replaced by single spaces, and surrounding whitespace
    /// trimmed — the form header grammars are defined against (RFC 3261 §7.3.1).
    ///
    /// Borrows when the value contains no folding, which is the common case.
    #[must_use]
    pub fn value(&self) -> Cow<'_, [u8]> {
        let raw = self.raw_value();
        if raw.iter().any(|&b| b == b'\r' || b == b'\n') {
            let mut out = Vec::with_capacity(raw.len());
            let mut i = 0;
            while let Some(&b) = raw.get(i) {
                if b == b'\r' && raw.get(i + 1) == Some(&b'\n') {
                    // A fold: the CRLF and the whitespace run after it collapse to one SP.
                    let mut j = i + 2;
                    while matches!(raw.get(j), Some(b' ' | b'\t')) {
                        j += 1;
                    }
                    out.push(b' ');
                    i = j;
                } else {
                    out.push(b);
                    i += 1;
                }
            }
            Cow::Owned(trim(&out).to_vec())
        } else {
            Cow::Borrowed(trim(raw))
        }
    }

    /// How many address values this row carries.
    ///
    /// The count uses the field's shared address grammar. It is useful for projecting a stable
    /// wire-order index across repeated rows without decoding or searching for value bytes.
    pub fn address_value_count(&self) -> Result<usize, AddressEditError> {
        self.address_layout().map(|layout| layout.spans.len())
    }

    /// Replace the URI in one address value, indexed within this header row.
    ///
    /// Only the parser-owned URI span changes. The display name may contain identical URI bytes;
    /// it is never considered because this operation consumes grammar ranges rather than searching
    /// the field. Failures leave the header unchanged.
    pub fn replace_address_uri(
        &mut self,
        value_index: usize,
        uri: &Uri,
    ) -> Result<(), AddressEditError> {
        let encoded = validate_replacement_uri(uri)?;
        let layout = self.address_layout()?;
        let span = layout
            .spans
            .get(value_index)
            .ok_or(AddressEditError::IndexOutOfRange { index: value_index })?;
        let raw_span =
            project_range(&layout, &span.uri).ok_or_else(|| malformed_address(self.name()))?;
        let rewritten = self
            .with_value_span_replaced(&raw_span, &encoded)
            .ok_or_else(|| malformed_address(self.name()))?;
        let candidate = rewritten.address_layout()?;
        if candidate.spans.len() != layout.spans.len() {
            return Err(malformed_address(self.name()));
        }
        let candidate_span = candidate
            .spans
            .get(value_index)
            .ok_or_else(|| malformed_address(self.name()))?;
        let candidate_raw_span = project_range(&candidate, &candidate_span.uri)
            .ok_or_else(|| malformed_address(self.name()))?;
        if rewritten.raw_value().get(candidate_raw_span) != Some(encoded.as_ref()) {
            return Err(malformed_address(self.name()));
        }
        *self = rewritten;
        Ok(())
    }

    /// Replace one address's display name, brackets and URI as one parser-owned span.
    ///
    /// The replacement always uses unambiguous name-address form. A present display name is
    /// quoted and escaped here; every byte outside the retained presentation span stays exact.
    /// Failures leave the header unchanged.
    pub fn replace_address_presentation(
        &mut self,
        value_index: usize,
        display_name: Option<&str>,
        uri: &Uri,
    ) -> Result<(), AddressEditError> {
        let encoded = encode_address_presentation(display_name, uri)?;
        let layout = self.address_layout()?;
        let span = layout
            .spans
            .get(value_index)
            .ok_or(AddressEditError::IndexOutOfRange { index: value_index })?;
        let raw_span = project_range(&layout, &span.presentation)
            .ok_or_else(|| malformed_address(self.name()))?;
        let rewritten = self
            .with_value_span_replaced(&raw_span, &encoded)
            .ok_or_else(|| malformed_address(self.name()))?;
        let candidate = rewritten.address_layout()?;
        if candidate.spans.len() != layout.spans.len() {
            return Err(malformed_address(self.name()));
        }
        let candidate_span = candidate
            .spans
            .get(value_index)
            .ok_or_else(|| malformed_address(self.name()))?;
        let candidate_raw_span = project_range(&candidate, &candidate_span.presentation)
            .ok_or_else(|| malformed_address(self.name()))?;
        if rewritten.raw_value().get(candidate_raw_span) != Some(encoded.as_ref()) {
            return Err(malformed_address(self.name()));
        }
        *self = rewritten;
        Ok(())
    }

    /// How many complete Warning values this row carries.
    ///
    /// The count uses the field's shared Warning grammar and therefore rejects an incomplete
    /// code, missing agent or malformed quoted text instead of counting delimiters locally.
    pub fn warning_value_count(&self) -> Result<usize, WarningEditError> {
        self.warning_layout().map(|layout| layout.spans.len())
    }

    /// Replace one Warning agent with an RFC 3261 token pseudonym.
    ///
    /// Only the parser-retained agent range changes. The code, separator spaces, quoted text,
    /// folding and list layout stay byte-identical. Failures leave the header unchanged.
    pub fn replace_warning_agent_with_pseudonym(
        &mut self,
        value_index: usize,
        pseudonym: &[u8],
    ) -> Result<(), WarningEditError> {
        validate_warning_pseudonym(pseudonym)?;
        let layout = self.warning_layout()?;
        let span = layout
            .spans
            .get(value_index)
            .ok_or(WarningEditError::IndexOutOfRange { index: value_index })?;
        let raw_span = project_source_range(&layout.source_map, layout.raw_len, &span.agent)
            .ok_or_else(malformed_warning)?;
        let rewritten = self
            .with_value_span_replaced(&raw_span, pseudonym)
            .ok_or_else(malformed_warning)?;
        let candidate = rewritten.warning_layout()?;
        if candidate.spans.len() != layout.spans.len() {
            return Err(malformed_warning());
        }
        let candidate_span = candidate
            .spans
            .get(value_index)
            .ok_or_else(malformed_warning)?;
        let candidate_raw_span = project_source_range(
            &candidate.source_map,
            candidate.raw_len,
            &candidate_span.agent,
        )
        .ok_or_else(malformed_warning)?;
        if rewritten.raw_value().get(candidate_raw_span) != Some(pseudonym) {
            return Err(malformed_warning());
        }
        *self = rewritten;
        Ok(())
    }

    /// Return this row with one address value removed.
    ///
    /// `Ok(None)` means the selected address was the row's sole value, so the containing header
    /// collection must remove the field line. Returning a new row keeps this operation atomic and
    /// gives standalone [`Header`] users an honest representation of row absence.
    pub fn without_address_value(
        &self,
        value_index: usize,
    ) -> Result<Option<Self>, AddressEditError> {
        let layout = self.address_layout()?;
        let selected = layout
            .spans
            .get(value_index)
            .ok_or(AddressEditError::IndexOutOfRange { index: value_index })?;
        if layout.spans.len() == 1 {
            return Ok(None);
        }

        let unfolded = if let Some(next) = layout.spans.get(value_index.saturating_add(1)) {
            selected.item.start..next.item.start
        } else {
            let previous = value_index
                .checked_sub(1)
                .and_then(|index| layout.spans.get(index))
                .ok_or_else(|| malformed_address(self.name()))?;
            previous.part.end..selected.item.end
        };
        let raw_span =
            project_range(&layout, &unfolded).ok_or_else(|| malformed_address(self.name()))?;
        self.with_value_span_replaced(&raw_span, &[])
            .map(Some)
            .ok_or_else(|| malformed_address(self.name()))
    }

    fn address_layout(&self) -> Result<AddressLayout, AddressEditError> {
        let (header, list) = address_grammar(self.name())?;
        let raw = self.raw_value();
        let (unfolded, source_map) = unfold_with_source_map(raw);
        let spans = value_spans(&unfolded, header, list).map_err(AddressEditError::Malformed)?;
        Ok(AddressLayout {
            spans,
            source_map,
            raw_len: raw.len(),
        })
    }

    fn warning_layout(&self) -> Result<WarningLayout, WarningEditError> {
        if self.name() != &HeaderName::Warning {
            return Err(malformed_warning());
        }
        let raw = self.raw_value();
        let (unfolded, source_map) = unfold_with_source_map(raw);
        let spans = warning_value_spans(&unfolded).map_err(WarningEditError::Malformed)?;
        Ok(WarningLayout {
            spans,
            source_map,
            raw_len: raw.len(),
        })
    }

    fn with_value_span_replaced(&self, span: &Range<usize>, replacement: &[u8]) -> Option<Self> {
        let repr = match &self.repr {
            HeaderRepr::Wire { line, value_offset } => {
                let start = value_offset.checked_add(span.start)?;
                let end = value_offset.checked_add(span.end)?;
                HeaderRepr::Wire {
                    line: replace_byte_span(line, &(start..end), replacement)?,
                    value_offset: *value_offset,
                }
            }
            HeaderRepr::Built { value } => HeaderRepr::Built {
                value: replace_byte_span(value, span, replacement)?,
            },
        };
        Some(Self {
            name: self.name.clone(),
            repr,
        })
    }

    /// Write this header as a field line, without the terminating CRLF.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        match &self.repr {
            HeaderRepr::Wire { line, .. } => out.extend_from_slice(line),
            HeaderRepr::Built { value } => {
                out.extend_from_slice(self.name.canonical());
                out.extend_from_slice(b": ");
                out.extend_from_slice(value);
            }
        }
    }
}

fn trim(mut b: &[u8]) -> &[u8] {
    while let Some((first, rest)) = b.split_first() {
        if matches!(first, b' ' | b'\t') {
            b = rest;
        } else {
            break;
        }
    }
    while let Some((last, rest)) = b.split_last() {
        if matches!(last, b' ' | b'\t') {
            b = rest;
        } else {
            break;
        }
    }
    b
}

/// The ordered header collection.
///
/// Order is preserved absolutely, including the relative order of same-named headers. `Via`
/// order determines where a response goes, so nothing here ever sorts or deduplicates.
#[derive(Debug, Clone, Default)]
pub struct Headers {
    entries: Vec<Header>,
}

impl Headers {
    /// An empty collection.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many header fields are present.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no headers.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Append a header, keeping any existing ones of the same name.
    pub fn push(&mut self, header: Header) {
        self.entries.push(header);
    }

    /// Insert a header at the front — where a new `Via` goes.
    pub fn push_front(&mut self, header: Header) {
        self.entries.insert(0, header);
    }

    /// Every header, in wire order.
    pub fn iter(&self) -> impl Iterator<Item = &Header> {
        self.entries.iter()
    }

    /// The first header with this name.
    #[must_use]
    pub fn get(&self, name: &HeaderName) -> Option<&Header> {
        self.entries.iter().find(|h| h.name() == name)
    }

    /// Every header with this name, in wire order.
    pub fn get_all<'a>(&'a self, name: &'a HeaderName) -> impl Iterator<Item = &'a Header> {
        self.entries.iter().filter(move |h| h.name() == name)
    }

    /// How many headers carry this name.
    #[must_use]
    pub fn count(&self, name: &HeaderName) -> usize {
        self.entries.iter().filter(|h| h.name() == name).count()
    }

    /// Remove every header with this name, returning how many went.
    pub fn remove_all(&mut self, name: &HeaderName) -> usize {
        let before = self.entries.len();
        self.entries.retain(|h| h.name() != name);
        before - self.entries.len()
    }

    /// Remove the **topmost** header with this name and return it.
    ///
    /// The one a forwarding element needs constantly: RFC 3261 §16.7 step 2 has a proxy remove the
    /// topmost `Via` from a response before forwarding it, and §16.6 has it push its own onto a
    /// request. Order is semantic for `Via`, `Route`, `Record-Route` and `Path` — it *is* the
    /// routing — so this is an exact position rather than a set operation, and everything else
    /// keeps its place.
    pub fn remove_first(&mut self, name: &HeaderName) -> Option<Header> {
        let index = self.entries.iter().position(|h| h.name() == name)?;
        Some(self.entries.remove(index))
    }

    /// Insert a header at an absolute position.
    ///
    /// An index past the end **appends** rather than panicking. This crate parses hostile input and
    /// a caller's index is often derived from it; a panic here would be a remote denial of service
    /// reachable through arithmetic, which is exactly the class of bug the builders exist to make
    /// unrepresentable.
    pub fn insert(&mut self, index: usize, header: Header) {
        let index = index.min(self.entries.len());
        self.entries.insert(index, header);
    }

    /// Keep the headers a predicate accepts, in place and in order.
    ///
    /// The general case behind [`Headers::remove_all`], for the filters a forwarding element writes
    /// that are not "by name" — stripping hop-by-hop headers, dropping a `Route` that names this
    /// proxy, removing everything a policy did not whitelist.
    pub fn retain(&mut self, f: impl FnMut(&Header) -> bool) {
        self.entries.retain(f);
    }

    /// Replace one address URI by its flattened wire-order value index.
    ///
    /// Repeated rows and comma-joined values share one zero-based index space. Every matching row
    /// is parsed before mutation, so a malformed later row cannot leave a partial edit behind.
    pub fn replace_address_uri(
        &mut self,
        name: &HeaderName,
        value_index: usize,
        uri: &Uri,
    ) -> Result<(), AddressEditError> {
        address_grammar(name)?;
        validate_replacement_uri(uri)?;
        let rows = self.address_rows(name)?;
        let (entry_index, row_index) = locate_address_value(&rows, value_index)?;
        let header = self
            .entries
            .get_mut(entry_index)
            .ok_or(AddressEditError::IndexOutOfRange { index: value_index })?;
        header.replace_address_uri(row_index, uri)
    }

    /// Replace one address presentation by its flattened wire-order value index.
    ///
    /// Repeated rows and comma-joined values share one zero-based index space. Every matching row
    /// is parsed before mutation, so a malformed later row cannot leave a partial edit behind.
    pub fn replace_address_presentation(
        &mut self,
        name: &HeaderName,
        value_index: usize,
        display_name: Option<&str>,
        uri: &Uri,
    ) -> Result<(), AddressEditError> {
        address_grammar(name)?;
        encode_address_presentation(display_name, uri)?;
        let rows = self.address_rows(name)?;
        let (entry_index, row_index) = locate_address_value(&rows, value_index)?;
        let header = self
            .entries
            .get_mut(entry_index)
            .ok_or(AddressEditError::IndexOutOfRange { index: value_index })?;
        header.replace_address_presentation(row_index, display_name, uri)
    }

    /// Replace one Warning agent by its flattened wire-order value index.
    ///
    /// Repeated rows and comma-joined values share one zero-based index space. Every Warning row
    /// is parsed before mutation, so a malformed later row cannot leave a partial edit behind.
    pub fn replace_warning_agent_with_pseudonym(
        &mut self,
        value_index: usize,
        pseudonym: &[u8],
    ) -> Result<(), WarningEditError> {
        validate_warning_pseudonym(pseudonym)?;
        let rows = self.warning_rows()?;
        let (entry_index, row_index) = locate_flattened_value(&rows, value_index)
            .map_err(|()| WarningEditError::IndexOutOfRange { index: value_index })?;
        let header = self
            .entries
            .get_mut(entry_index)
            .ok_or(WarningEditError::IndexOutOfRange { index: value_index })?;
        header.replace_warning_agent_with_pseudonym(row_index, pseudonym)
    }

    /// Remove one address value by its flattened wire-order index.
    ///
    /// If it was a row's sole value, that exact field line is removed. Otherwise only the value
    /// and one adjacent list delimiter are removed; all surviving wire bytes retain their order.
    pub fn remove_address_value(
        &mut self,
        name: &HeaderName,
        value_index: usize,
    ) -> Result<(), AddressEditError> {
        address_grammar(name)?;
        let rows = self.address_rows(name)?;
        let (entry_index, row_index) = locate_address_value(&rows, value_index)?;
        let replacement = self
            .entries
            .get(entry_index)
            .ok_or(AddressEditError::IndexOutOfRange { index: value_index })?
            .without_address_value(row_index)?;
        if let Some(header) = replacement {
            let slot = self
                .entries
                .get_mut(entry_index)
                .ok_or(AddressEditError::IndexOutOfRange { index: value_index })?;
            *slot = header;
        } else {
            self.entries.remove(entry_index);
        }
        Ok(())
    }

    fn address_rows(&self, name: &HeaderName) -> Result<Vec<(usize, usize)>, AddressEditError> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, header)| header.name() == name)
            .map(|(index, header)| header.address_value_count().map(|count| (index, count)))
            .collect()
    }

    fn warning_rows(&self) -> Result<Vec<(usize, usize)>, WarningEditError> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, header)| header.name() == &HeaderName::Warning)
            .map(|(index, header)| header.warning_value_count().map(|count| (index, count)))
            .collect()
    }

    /// The first value with this name, unfolded.
    #[must_use]
    pub fn value(&self, name: &HeaderName) -> Option<Cow<'_, [u8]>> {
        self.get(name).map(Header::value)
    }

    /// Write every header, each followed by CRLF.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        for h in &self.entries {
            h.write_to(out);
            out.extend_from_slice(b"\r\n");
        }
    }
}

fn address_grammar(name: &HeaderName) -> Result<(&'static str, bool), AddressEditError> {
    match name {
        HeaderName::From => Ok(("From", false)),
        HeaderName::To => Ok(("To", false)),
        HeaderName::Contact => Ok(("Contact", true)),
        HeaderName::Route => Ok(("Route", true)),
        HeaderName::RecordRoute => Ok(("Record-Route", true)),
        HeaderName::Path => Ok(("Path", true)),
        HeaderName::ServiceRoute => Ok(("Service-Route", true)),
        HeaderName::PAssertedIdentity => Ok(("P-Asserted-Identity", true)),
        HeaderName::PPreferredIdentity => Ok(("P-Preferred-Identity", true)),
        _ => Err(AddressEditError::UnsupportedHeader),
    }
}

fn malformed_address(name: &HeaderName) -> AddressEditError {
    let header = address_grammar(name).map_or("address", |(header, _)| header);
    AddressEditError::Malformed(HeaderError::Syntax { header })
}

fn malformed_warning() -> WarningEditError {
    WarningEditError::Malformed(HeaderError::Syntax { header: "Warning" })
}

fn validate_warning_pseudonym(pseudonym: &[u8]) -> Result<(), WarningEditError> {
    if pseudonym.is_empty() || !pseudonym.iter().copied().all(is_token_char) {
        return Err(WarningEditError::InvalidPseudonym);
    }
    Ok(())
}

fn validate_replacement_uri(uri: &Uri) -> Result<Bytes, AddressEditError> {
    let encoded = uri.to_bytes();
    Uri::parse(encoded.clone()).map_err(AddressEditError::InvalidUri)?;
    Ok(encoded)
}

fn encode_address_presentation(
    display_name: Option<&str>,
    uri: &Uri,
) -> Result<Bytes, AddressEditError> {
    let uri = validate_replacement_uri(uri)?;
    let mut encoded = Vec::new();
    if let Some(display_name) = display_name {
        if display_name
            .as_bytes()
            .iter()
            .any(|byte| *byte < 0x20 || *byte == 0x7f)
        {
            return Err(AddressEditError::InvalidDisplayName);
        }
        encoded.push(b'"');
        for byte in display_name.as_bytes() {
            if matches!(byte, b'"' | b'\\') {
                encoded.push(b'\\');
            }
            encoded.push(*byte);
        }
        encoded.extend_from_slice(b"\" ");
    }
    encoded.push(b'<');
    encoded.extend_from_slice(&uri);
    encoded.push(b'>');
    Ok(Bytes::from(encoded))
}

fn locate_address_value(
    rows: &[(usize, usize)],
    value_index: usize,
) -> Result<(usize, usize), AddressEditError> {
    locate_flattened_value(rows, value_index)
        .map_err(|()| AddressEditError::IndexOutOfRange { index: value_index })
}

fn locate_flattened_value(
    rows: &[(usize, usize)],
    value_index: usize,
) -> Result<(usize, usize), ()> {
    let mut first = 0usize;
    for &(entry_index, count) in rows {
        let end = first.checked_add(count).ok_or(())?;
        if value_index < end {
            return Ok((entry_index, value_index - first));
        }
        first = end;
    }
    Err(())
}

fn unfold_with_source_map(raw: &[u8]) -> (Vec<u8>, Vec<Range<usize>>) {
    let mut unfolded = Vec::with_capacity(raw.len());
    let mut source_map = Vec::with_capacity(raw.len());
    let mut i = 0usize;
    while let Some(&byte) = raw.get(i) {
        if byte == b'\r'
            && raw.get(i + 1) == Some(&b'\n')
            && matches!(raw.get(i + 2), Some(b' ' | b'\t'))
        {
            let mut end = i + 2;
            while matches!(raw.get(end), Some(b' ' | b'\t')) {
                end += 1;
            }
            unfolded.push(b' ');
            source_map.push(i..end);
            i = end;
        } else {
            unfolded.push(byte);
            source_map.push(i..i + 1);
            i += 1;
        }
    }
    (unfolded, source_map)
}

fn project_range(layout: &AddressLayout, span: &Range<usize>) -> Option<Range<usize>> {
    project_source_range(&layout.source_map, layout.raw_len, span)
}

fn project_source_range(
    source_map: &[Range<usize>],
    raw_len: usize,
    span: &Range<usize>,
) -> Option<Range<usize>> {
    if span.start > span.end || span.end > source_map.len() {
        return None;
    }
    let start = source_boundary(source_map, raw_len, span.start)?;
    let end = source_boundary(source_map, raw_len, span.end)?;
    (start <= end).then_some(start..end)
}

fn source_boundary(source_map: &[Range<usize>], raw_len: usize, position: usize) -> Option<usize> {
    if position == source_map.len() {
        Some(raw_len)
    } else {
        source_map.get(position).map(|source| source.start)
    }
}

/// A parsed request.
#[derive(Debug, Clone)]
pub struct Request {
    /// The method.
    pub method: Method,
    /// The Request-URI.
    ///
    /// Use [`Request::set_uri`] to mutate this value. Assigning the field directly cannot update
    /// the parser-retained start-line span and would replay stale wire bytes.
    pub uri: Uri,
    /// The protocol version.
    pub version: Version,
    /// The headers, in wire order.
    pub headers: Headers,
    body: Bytes,
    raw_start_line: Option<Bytes>,
    raw_uri_span: Option<Range<usize>>,
}

/// A parsed response.
#[derive(Debug, Clone)]
pub struct Response {
    /// The protocol version.
    pub version: Version,
    /// The status code.
    pub status: StatusCode,
    /// The reason phrase, which may be empty (RFC 4475 §3.1.1.13) and may contain spaces.
    pub reason: Bytes,
    /// The headers, in wire order.
    pub headers: Headers,
    body: Bytes,
    raw_start_line: Option<Bytes>,
}

/// A request or a response.
#[derive(Debug, Clone)]
pub enum Message {
    /// A request.
    Request(Request),
    /// A response.
    Response(Response),
}

impl Request {
    pub(crate) fn from_wire(
        method: Method,
        uri: Uri,
        version: Version,
        raw_start_line: Bytes,
        raw_uri_span: Range<usize>,
        headers: Headers,
        body: Bytes,
    ) -> Self {
        Self {
            method,
            uri,
            version,
            headers,
            body,
            raw_start_line: Some(raw_start_line),
            raw_uri_span: Some(raw_uri_span),
        }
    }

    /// Replace the Request-URI without rebuilding a parsed start line.
    ///
    /// Retargeting logic must use this rather than assigning the public field directly. For a
    /// parsed request, only the parser-owned URI span changes; method spelling, separators and
    /// SIP-version bytes stay exact. Constructed requests retain deterministic serialization.
    /// The replacement is validated from its serialized bytes before either representation is
    /// changed, so a failure is atomic.
    pub fn set_uri(&mut self, uri: Uri) -> Result<(), crate::error::UriError> {
        let encoded = uri.to_bytes();
        Uri::parse(encoded.clone())?;

        let rewritten = match (&self.raw_start_line, &self.raw_uri_span) {
            (Some(raw), Some(span)) => Some(
                replace_byte_span(raw, span, &encoded)
                    .ok_or(crate::error::UriError::RetainedSpan)?,
            ),
            (None, None) => None,
            _ => return Err(crate::error::UriError::RetainedSpan),
        };

        if let Some(raw) = rewritten {
            let start = self
                .raw_uri_span
                .as_ref()
                .map(|span| span.start)
                .ok_or(crate::error::UriError::RetainedSpan)?;
            let end = start
                .checked_add(encoded.len())
                .ok_or(crate::error::UriError::RetainedSpan)?;
            self.raw_start_line = Some(raw);
            self.raw_uri_span = Some(start..end);
        } else {
            self.raw_start_line = None;
            self.raw_uri_span = None;
        }
        self.uri = uri;
        Ok(())
    }

    /// Build a request.
    #[must_use]
    pub fn new(method: Method, uri: Uri) -> Self {
        Self {
            method,
            uri,
            version: Version::Sip20,
            headers: Headers::new(),
            body: Bytes::new(),
            raw_start_line: None,
            raw_uri_span: None,
        }
    }

    /// The message body.
    #[must_use]
    pub fn body(&self) -> &Bytes {
        &self.body
    }

    /// Replace the body. The caller is responsible for `Content-Length`.
    pub fn set_body(&mut self, body: Bytes) {
        self.body = body;
    }

    /// Serialize.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        if let Some(raw) = &self.raw_start_line {
            out.extend_from_slice(raw);
        } else {
            out.extend_from_slice(self.method.as_bytes());
            out.push(b' ');
            self.uri.write_to(out);
            out.push(b' ');
            out.extend_from_slice(self.version.as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        self.headers.write_to(out);
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
    }
}

fn replace_byte_span(source: &Bytes, span: &Range<usize>, replacement: &[u8]) -> Option<Bytes> {
    let prefix = source.get(..span.start)?;
    let suffix = source.get(span.end..)?;
    let capacity = prefix
        .len()
        .checked_add(replacement.len())?
        .checked_add(suffix.len())?;
    let mut out = Vec::with_capacity(capacity);
    out.extend_from_slice(prefix);
    out.extend_from_slice(replacement);
    out.extend_from_slice(suffix);
    Some(Bytes::from(out))
}

impl Response {
    pub(crate) fn from_wire(
        version: Version,
        status: StatusCode,
        reason: Bytes,
        raw_start_line: Bytes,
        headers: Headers,
        body: Bytes,
    ) -> Self {
        Self {
            version,
            status,
            reason,
            headers,
            body,
            raw_start_line: Some(raw_start_line),
        }
    }

    /// Build a response.
    #[must_use]
    pub fn new(status: StatusCode, reason: impl Into<Bytes>) -> Self {
        Self {
            version: Version::Sip20,
            status,
            reason: reason.into(),
            headers: Headers::new(),
            body: Bytes::new(),
            raw_start_line: None,
        }
    }

    /// The message body.
    #[must_use]
    pub fn body(&self) -> &Bytes {
        &self.body
    }

    /// Replace the body. The caller is responsible for `Content-Length`.
    pub fn set_body(&mut self, body: Bytes) {
        self.body = body;
    }

    /// Serialize.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        if let Some(raw) = &self.raw_start_line {
            out.extend_from_slice(raw);
        } else {
            out.extend_from_slice(self.version.as_bytes());
            out.push(b' ');
            out.extend_from_slice(self.status.to_string().as_bytes());
            out.push(b' ');
            out.extend_from_slice(&self.reason);
        }
        out.extend_from_slice(b"\r\n");
        self.headers.write_to(out);
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.body);
    }
}

impl Message {
    /// The headers, whichever kind of message this is.
    #[must_use]
    pub fn headers(&self) -> &Headers {
        match self {
            Self::Request(r) => &r.headers,
            Self::Response(r) => &r.headers,
        }
    }

    /// The headers, mutably.
    pub fn headers_mut(&mut self) -> &mut Headers {
        match self {
            Self::Request(r) => &mut r.headers,
            Self::Response(r) => &mut r.headers,
        }
    }

    /// The body.
    #[must_use]
    pub fn body(&self) -> &Bytes {
        match self {
            Self::Request(r) => r.body(),
            Self::Response(r) => r.body(),
        }
    }

    /// The request, if this is one.
    #[must_use]
    pub fn as_request(&self) -> Option<&Request> {
        match self {
            Self::Request(r) => Some(r),
            Self::Response(_) => None,
        }
    }

    /// The response, if this is one.
    #[must_use]
    pub fn as_response(&self) -> Option<&Response> {
        match self {
            Self::Response(r) => Some(r),
            Self::Request(_) => None,
        }
    }

    /// Serialize.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        match self {
            Self::Request(r) => r.write_to(out),
            Self::Response(r) => r.write_to(out),
        }
    }

    /// Serialize to bytes.
    ///
    /// A parsed, unmodified message reproduces its input exactly.
    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        let mut out = Vec::new();
        self.write_to(&mut out);
        Bytes::from(out)
    }
}

/// A header value that parses into a typed form.
pub trait TypedHeader: Sized {
    /// The header this type reads.
    const NAME: HeaderName;

    /// Whether [`Headers::typed_all`] must collect and validate the complete field before yielding.
    ///
    /// The default keeps ordinary headers streaming and allocation-free across rows. A field with
    /// message-wide constraints opts in and implements [`Self::validate_list`].
    const VALIDATE_LIST: bool = false;

    /// Parse one header value. The value arrives unfolded and trimmed.
    fn decode(value: &[u8]) -> Result<Self, HeaderError>;

    /// Parse every value in one header row.
    ///
    /// RFC 3261 §7.3 makes a comma-joined row exactly equivalent to the same values on
    /// separate rows for headers whose grammar is a comma-separated list; those headers
    /// override this. Everything else carries exactly one value per row.
    fn decode_list(value: &[u8]) -> Result<Vec<Self>, HeaderError> {
        Self::decode(value).map(|one| vec![one])
    }

    /// Validate all decoded values of this field across the complete message.
    ///
    /// Most SIP list fields have no message-wide constraint, so the default accepts every
    /// sequence. A field whose grammar constrains the number or relationship of values can
    /// override this; [`Headers::typed_all`] calls it after expanding every comma-separated row
    /// and repeated field line into wire order.
    fn validate_list(_values: &[&Self]) -> Result<(), HeaderError> {
        Ok(())
    }
}

struct TypedAll<'a, H: TypedHeader> {
    entries: std::slice::Iter<'a, Header>,
    row: std::vec::IntoIter<H>,
    validated: Option<std::vec::IntoIter<Result<H, HeaderError>>>,
}

impl<'a, H: TypedHeader> TypedAll<'a, H> {
    fn new(headers: &'a Headers) -> Self {
        let validated = H::VALIDATE_LIST.then(|| {
            let mut decoded: Vec<Result<H, HeaderError>> = headers
                .entries
                .iter()
                .filter(|header| header.name() == &H::NAME)
                .flat_map(|header| match H::decode_list(&header.value()) {
                    Ok(values) => values.into_iter().map(Ok).collect::<Vec<_>>(),
                    Err(error) => vec![Err(error)],
                })
                .collect();

            // A constrained field is useful only as one complete validated value. Collapse any
            // row-level failure before yielding so a caller cannot observe neighboring elements
            // that never passed the field-wide relationship. An empty iterator still means
            // absence rather than an empty field value.
            let decode_error = decoded.iter().find_map(|result| match result {
                Ok(_) => None,
                Err(error) => Some(error.clone()),
            });
            if let Some(error) = decode_error {
                decoded = vec![Err(error)];
            } else if !decoded.is_empty() {
                let values: Vec<&H> = decoded
                    .iter()
                    .filter_map(|result| result.as_ref().ok())
                    .collect();
                if let Err(error) = H::validate_list(&values) {
                    decoded = vec![Err(error)];
                }
            }
            decoded.into_iter()
        });

        Self {
            entries: headers.entries.iter(),
            row: Vec::new().into_iter(),
            validated,
        }
    }
}

impl<H: TypedHeader> Iterator for TypedAll<'_, H> {
    type Item = Result<H, HeaderError>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(validated) = &mut self.validated {
            return validated.next();
        }

        loop {
            if let Some(value) = self.row.next() {
                return Some(Ok(value));
            }
            let header = self.entries.find(|header| header.name() == &H::NAME)?;
            match H::decode_list(&header.value()) {
                Ok(values) => self.row = values.into_iter(),
                Err(error) => return Some(Err(error)),
            }
        }
    }
}

impl Headers {
    /// Parse the first header of this type.
    ///
    /// Returns `None` when the header is absent and `Some(Err(..))` when it is present and
    /// malformed. Collapsing those two is how implementations end up treating a corrupt
    /// `CSeq` as a missing one.
    #[must_use]
    pub fn typed<H: TypedHeader>(&self) -> Option<Result<H, HeaderError>> {
        self.get(&H::NAME).map(|h| H::decode(&h.value()))
    }

    /// Parse every header of this type, in wire order, yielding each element of a
    /// comma-separated row separately — one row of `n` values and `n` rows of one value are
    /// the same message (RFC 3261 §7.3).
    pub fn typed_all<'a, H: TypedHeader + 'a>(
        &'a self,
    ) -> impl Iterator<Item = Result<H, HeaderError>> + 'a {
        TypedAll::new(self)
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
    fn unfolding_collapses_continuations_to_a_single_space() {
        let line = Bytes::from_static(b"Subject: one\r\n  two\r\n\tthree");
        let h = Header::from_wire(HeaderName::Subject, line, 9);
        assert_eq!(h.value().as_ref(), b"one two three");
        // The raw form keeps the folding, so forwarding is byte-exact.
        assert_eq!(h.raw_value(), b"one\r\n  two\r\n\tthree");
    }

    #[test]
    fn unfolded_value_borrows_when_there_is_no_folding() {
        let line = Bytes::from_static(b"Subject: plain");
        let h = Header::from_wire(HeaderName::Subject, line, 9);
        assert!(matches!(h.value(), Cow::Borrowed(_)));
    }

    #[test]
    fn status_code_range_is_enforced() {
        assert!(StatusCode::new(99).is_none());
        assert!(StatusCode::new(700).is_none());
        assert_eq!(StatusCode::new(200).map(StatusCode::code), Some(200));
        assert!(StatusCode::new(180).unwrap().is_provisional());
        assert!(StatusCode::new(200).unwrap().is_success());
        assert!(StatusCode::new(486).unwrap().is_final());
    }

    #[test]
    fn methods_compare_case_sensitively() {
        // RFC 3261 7.1: method names are case-sensitive, so this is a different method and
        // not a sloppy spelling of INVITE.
        assert_ne!(
            Method::parse(&Bytes::from_static(b"Invite")),
            Method::Invite
        );
        assert_eq!(
            Method::parse(&Bytes::from_static(b"INVITE")),
            Method::Invite
        );
    }

    /// The story's failing-first test.
    ///
    /// RFC 3261 §16.7 step 2 has a proxy remove the topmost `Via` from a response and forward what
    /// is left. "Topmost" is exact: removing the wrong one, or removing all of them, sends the
    /// response to the wrong element or to nowhere.
    #[test]
    fn remove_first_takes_only_the_topmost_via() {
        let mut headers = Headers::new();
        for value in [&b"first"[..], b"second", b"third"] {
            headers.push(Header::new_unchecked(
                HeaderName::Via,
                Bytes::copy_from_slice(value),
            ));
        }
        // A header of another name between them, to catch an implementation that counts positions
        // among matching headers rather than among all of them.
        headers.insert(
            1,
            Header::new_unchecked(HeaderName::Route, Bytes::from_static(b"r")),
        );

        let taken = headers.remove_first(&HeaderName::Via).expect("a Via");
        assert_eq!(taken.value().as_ref(), b"first");
        assert_eq!(
            headers
                .get_all(&HeaderName::Via)
                .map(|h| h.value().to_vec())
                .collect::<Vec<_>>(),
            vec![b"second".to_vec(), b"third".to_vec()],
            "the remaining Vias keep their order"
        );
        assert_eq!(
            headers.iter().map(|h| h.name().clone()).collect::<Vec<_>>(),
            vec![HeaderName::Route, HeaderName::Via, HeaderName::Via],
            "and every other header stays where it was"
        );
    }

    #[test]
    fn remove_first_on_a_name_that_is_absent_yields_nothing_and_changes_nothing() {
        let mut headers = Headers::new();
        headers.push(Header::new_unchecked(
            HeaderName::Via,
            Bytes::from_static(b"v"),
        ));
        assert!(headers.remove_first(&HeaderName::Route).is_none());
        assert_eq!(headers.len(), 1);
    }

    /// An index past the end appends. This crate parses hostile input, and a caller's index is
    /// often derived from it — a panic here would be a remote denial of service reachable through
    /// arithmetic.
    #[test]
    fn inserting_past_the_end_appends_rather_than_panicking() {
        let mut headers = Headers::new();
        headers.push(Header::new_unchecked(
            HeaderName::Via,
            Bytes::from_static(b"v"),
        ));
        headers.insert(
            9999,
            Header::new_unchecked(HeaderName::Route, Bytes::from_static(b"r")),
        );
        assert_eq!(headers.len(), 2);
        assert_eq!(
            headers.iter().last().map(|h| h.name().clone()),
            Some(HeaderName::Route)
        );
    }

    #[test]
    fn insert_places_a_header_at_an_absolute_position() {
        let mut headers = Headers::new();
        for name in [HeaderName::Via, HeaderName::To, HeaderName::From] {
            headers.push(Header::new_unchecked(name, Bytes::from_static(b"x")));
        }
        headers.insert(
            1,
            Header::new_unchecked(HeaderName::RecordRoute, Bytes::from_static(b"rr")),
        );
        assert_eq!(
            headers.iter().map(|h| h.name().clone()).collect::<Vec<_>>(),
            vec![
                HeaderName::Via,
                HeaderName::RecordRoute,
                HeaderName::To,
                HeaderName::From
            ]
        );
        // Zero is the front, which is `push_front`.
        headers.insert(
            0,
            Header::new_unchecked(HeaderName::Via, Bytes::from_static(b"newest")),
        );
        assert_eq!(headers.value(&HeaderName::Via).unwrap().as_ref(), b"newest");
    }

    /// The general case behind `remove_all`: a filter that is not "by name".
    #[test]
    fn retain_filters_in_place_and_keeps_order() {
        let mut headers = Headers::new();
        for (name, value) in [
            (HeaderName::Via, &b"keep"[..]),
            (HeaderName::Route, b"drop"),
            (HeaderName::Via, b"drop"),
            (HeaderName::To, b"keep"),
        ] {
            headers.push(Header::new_unchecked(name, Bytes::copy_from_slice(value)));
        }
        headers.retain(|header| header.value().as_ref() == b"keep");
        assert_eq!(
            headers.iter().map(|h| h.name().clone()).collect::<Vec<_>>(),
            vec![HeaderName::Via, HeaderName::To]
        );
    }

    #[test]
    fn header_order_is_preserved_including_duplicates() {
        let mut headers = Headers::new();
        headers.push(Header::new_unchecked(
            HeaderName::Via,
            Bytes::from_static(b"first"),
        ));
        headers.push(Header::new_unchecked(
            HeaderName::Route,
            Bytes::from_static(b"r"),
        ));
        headers.push(Header::new_unchecked(
            HeaderName::Via,
            Bytes::from_static(b"second"),
        ));

        let vias: Vec<_> = headers
            .get_all(&HeaderName::Via)
            .map(|h| h.value().to_vec())
            .collect();
        assert_eq!(vias, vec![b"first".to_vec(), b"second".to_vec()]);
        assert_eq!(headers.count(&HeaderName::Via), 2);

        // A new Via goes on the front, ahead of everything.
        headers.push_front(Header::new_unchecked(
            HeaderName::Via,
            Bytes::from_static(b"newest"),
        ));
        assert_eq!(headers.value(&HeaderName::Via).unwrap().as_ref(), b"newest");
    }

    /// RFC 3261 §7.3: `Contact: <a>, <b>` and two `Contact` rows are the same message, so
    /// iterating the typed values must yield the same elements either way.
    #[test]
    fn typed_all_yields_each_element_of_a_comma_separated_row() {
        use crate::headers::Contact;

        let mut headers = Headers::new();
        headers.push(Header::new_unchecked(
            HeaderName::Contact,
            Bytes::from_static(b"<sip:a@b.com>, <sip:c@d.com>"),
        ));
        headers.push(Header::new_unchecked(
            HeaderName::Contact,
            Bytes::from_static(b"<sip:e@f.org>"),
        ));

        let contacts: Vec<Contact> = headers
            .typed_all::<Contact>()
            .collect::<Result<_, _>>()
            .unwrap();
        let uris: Vec<_> = contacts.iter().map(|c| c.uri.to_bytes()).collect();
        assert_eq!(
            uris,
            vec![
                Bytes::from_static(b"sip:a@b.com"),
                Bytes::from_static(b"sip:c@d.com"),
                Bytes::from_static(b"sip:e@f.org"),
            ]
        );
    }

    #[test]
    fn typed_all_keeps_unconstrained_headers_lazy() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static DECODES: AtomicUsize = AtomicUsize::new(0);

        struct CountingSubject;

        impl TypedHeader for CountingSubject {
            const NAME: HeaderName = HeaderName::Subject;

            fn decode(_value: &[u8]) -> Result<Self, HeaderError> {
                DECODES.fetch_add(1, Ordering::SeqCst);
                Ok(Self)
            }
        }

        DECODES.store(0, Ordering::SeqCst);
        let mut headers = Headers::new();
        headers.push(Header::new_unchecked(
            HeaderName::Subject,
            Bytes::from_static(b"first"),
        ));
        headers.push(Header::new_unchecked(
            HeaderName::Subject,
            Bytes::from_static(b"second"),
        ));

        let mut values = headers.typed_all::<CountingSubject>();
        assert_eq!(DECODES.load(Ordering::SeqCst), 0);
        assert!(values.next().is_some_and(|value| value.is_ok()));
        assert_eq!(DECODES.load(Ordering::SeqCst), 1);
        drop(values);
        assert_eq!(DECODES.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn built_headers_serialize_canonically() {
        let mut headers = Headers::new();
        headers.push(Header::new_unchecked(
            HeaderName::MaxForwards,
            Bytes::from_static(b"70"),
        ));
        let mut out = Vec::new();
        headers.write_to(&mut out);
        assert_eq!(out, b"Max-Forwards: 70\r\n");
    }

    #[test]
    fn wire_headers_serialize_verbatim() {
        // Original spelling, compact form and odd spacing all survive.
        let line = Bytes::from_static(b"MaX-fOrWaRdS  :   0068");
        let h = Header::from_wire(HeaderName::MaxForwards, line.clone(), 17);
        let mut out = Vec::new();
        h.write_to(&mut out);
        assert_eq!(out, line);
        assert_eq!(h.value().as_ref(), b"0068");
    }
}
