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

use bytes::Bytes;

use crate::error::HeaderError;
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
        if raw.as_ref() == b"SIP/2.0" {
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

impl Header {
    /// Build a header.
    #[must_use]
    pub fn new(name: HeaderName, value: impl Into<Bytes>) -> Self {
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
        if first.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    while let Some((last, rest)) = b.split_last() {
        if last.is_ascii_whitespace() {
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

/// A parsed request.
#[derive(Debug, Clone)]
pub struct Request {
    /// The method.
    pub method: Method,
    /// The Request-URI.
    pub uri: Uri,
    /// The protocol version.
    pub version: Version,
    /// The headers, in wire order.
    pub headers: Headers,
    body: Bytes,
    raw_start_line: Option<Bytes>,
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
        }
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

    /// Parse one header value. The value arrives unfolded and trimmed.
    fn decode(value: &[u8]) -> Result<Self, HeaderError>;
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

    /// Parse every header of this type, in wire order.
    pub fn typed_all<H: TypedHeader>(&self) -> impl Iterator<Item = Result<H, HeaderError>> + '_ {
        // Bound the name to a local so the iterator can borrow it.
        self.entries
            .iter()
            .filter(|h| h.name() == &H::NAME)
            .map(|h| H::decode(&h.value()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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

    #[test]
    fn header_order_is_preserved_including_duplicates() {
        let mut headers = Headers::new();
        headers.push(Header::new(HeaderName::Via, Bytes::from_static(b"first")));
        headers.push(Header::new(HeaderName::Route, Bytes::from_static(b"r")));
        headers.push(Header::new(HeaderName::Via, Bytes::from_static(b"second")));

        let vias: Vec<_> = headers
            .get_all(&HeaderName::Via)
            .map(|h| h.value().to_vec())
            .collect();
        assert_eq!(vias, vec![b"first".to_vec(), b"second".to_vec()]);
        assert_eq!(headers.count(&HeaderName::Via), 2);

        // A new Via goes on the front, ahead of everything.
        headers.push_front(Header::new(HeaderName::Via, Bytes::from_static(b"newest")));
        assert_eq!(headers.value(&HeaderName::Via).unwrap().as_ref(), b"newest");
    }

    #[test]
    fn built_headers_serialize_canonically() {
        let mut headers = Headers::new();
        headers.push(Header::new(
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
