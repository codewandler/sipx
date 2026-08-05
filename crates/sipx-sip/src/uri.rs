//! SIP, SIPS and other URIs (RFC 3261 §19.1).

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use bytes::Bytes;
use thiserror::Error;

use crate::error::{BuildError, UriError};
use crate::escape;
use crate::params::{Param, Params};

/// A URI scheme.
///
/// Comparison is case-insensitive, but `sip` and `sips` are **never** equivalent
/// (RFC 3261 §19.1.4) — a secure URI is a different address, not a spelling variant.
#[derive(Debug, Clone)]
pub enum Scheme {
    /// `sip:`
    Sip,
    /// `sips:`
    Sips,
    /// `tel:` (RFC 3966).
    Tel,
    /// Any other scheme, retained verbatim.
    Other(Bytes),
}

/// Effective wire transport selected by a SIP/SIPS URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UriTransport {
    /// UDP datagrams.
    Udp,
    /// TCP stream.
    Tcp,
    /// TLS over TCP.
    Tls,
    /// SIP over WebSocket.
    Ws,
    /// SIP over secure WebSocket.
    Wss,
    /// SIP over QUIC.
    Quic,
}

impl UriTransport {
    /// Default port for this transport when the URI omits one.
    #[must_use]
    pub fn default_port(self) -> u16 {
        match self {
            Self::Udp | Self::Tcp => 5060,
            Self::Tls | Self::Quic => 5061,
            Self::Ws => 80,
            Self::Wss => 443,
        }
    }
}

/// Why a URI cannot select a safe wire transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum UriTransportError {
    /// The scheme is neither SIP nor SIPS.
    #[error("the URI is not a SIP or SIPS URI")]
    NotSip,
    /// The transport parameter is not implemented.
    #[error("the URI names an unsupported transport")]
    Unsupported,
    /// SIPS cannot be carried over UDP.
    #[error("a SIPS URI cannot select UDP")]
    SecureDatagram,
}

impl Scheme {
    #[must_use]
    fn parse(raw: &Bytes) -> Self {
        if escape::eq_ignore_ascii_case(raw, b"sip") {
            Self::Sip
        } else if escape::eq_ignore_ascii_case(raw, b"sips") {
            Self::Sips
        } else if escape::eq_ignore_ascii_case(raw, b"tel") {
            Self::Tel
        } else {
            Self::Other(raw.clone())
        }
    }

    /// The scheme as it should be written.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Sip => b"sip",
            Self::Sips => b"sips",
            Self::Tel => b"tel",
            Self::Other(raw) => raw,
        }
    }

    /// Whether this scheme implies a secure transport.
    #[must_use]
    pub fn is_secure(&self) -> bool {
        matches!(self, Self::Sips)
    }

    /// Whether this is `sip:` or `sips:`, and therefore has structured parts.
    #[must_use]
    pub fn is_sip(&self) -> bool {
        matches!(self, Self::Sip | Self::Sips)
    }

    #[must_use]
    fn equivalent(&self, other: &Self) -> bool {
        escape::eq_ignore_ascii_case(self.as_bytes(), other.as_bytes())
    }
}

/// A validated hostname.
///
/// The inner bytes are private and the only public constructor checks them. That is what
/// stops a caller from putting a CRLF in a host and injecting a header through the
/// Request-URI: without this, `Host::Name(b"evil\r\nInjected: yes")` would serialize into a
/// perfectly convincing forged request line.
#[derive(Debug, Clone)]
pub struct HostName(Bytes);

impl HostName {
    /// Validate a hostname.
    pub fn new(name: impl Into<Bytes>) -> Result<Self, BuildError> {
        let name = name.into();
        if name.is_empty() || !name.iter().all(|&b| is_host_char(b)) {
            return Err(BuildError::NotAToken { field: "host" });
        }
        Ok(Self(name))
    }

    pub(crate) fn new_unchecked(name: Bytes) -> Self {
        Self(name)
    }

    /// The hostname.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl PartialEq<&[u8]> for HostName {
    fn eq(&self, other: &&[u8]) -> bool {
        self.0 == *other
    }
}

impl PartialEq<&str> for HostName {
    fn eq(&self, other: &&str) -> bool {
        self.0 == other.as_bytes()
    }
}

/// The host part of a URI.
#[derive(Debug, Clone)]
pub enum Host {
    /// A hostname.
    Name(HostName),
    /// A literal IPv4 or IPv6 address.
    Ip(IpAddr),
}

impl Host {
    /// Parse a `host [ ":" port ]`, as it appears in a URI or in a `Via` sent-by.
    ///
    /// Shared with the `Via` header so that a hostname is validated the same way wherever it
    /// appears; a host that is rejected in a URI must not be accepted in a `Via`.
    pub fn parse_hostport(raw: &Bytes) -> Result<(Self, Option<u16>), UriError> {
        parse_hostport(raw)
    }

    /// The host as it should be written, without IPv6 brackets.
    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        match self {
            Self::Name(name) => name.0.clone(),
            Self::Ip(ip) => Bytes::from(ip.to_string()),
        }
    }

    /// Whether two hosts are the same.
    ///
    /// Hostnames compare case-insensitively. A hostname never matches an IP address, even the
    /// one it resolves to — RFC 3261 §19.1.4 is explicit, and a comparison that consulted DNS
    /// would be neither pure nor stable.
    #[must_use]
    pub fn equivalent(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Ip(a), Self::Ip(b)) => a == b,
            (Self::Name(a), Self::Name(b)) => escape::eq_ignore_ascii_case(&a.0, &b.0),
            _ => false,
        }
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => write!(f, "{}", String::from_utf8_lossy(&name.0)),
            Self::Ip(IpAddr::V6(ip)) => write!(f, "[{ip}]"),
            Self::Ip(ip) => write!(f, "{ip}"),
        }
    }
}

/// The structured parts of a `sip:` or `sips:` URI.
#[derive(Debug, Clone)]
struct SipParts {
    user: Option<Bytes>,
    /// Exact span of `user` in [`Uri::raw`]. Absent for a URI without userinfo and cleared when
    /// another structured mutation discards the verbatim form.
    raw_user_span: Option<std::ops::Range<usize>>,
    password: Option<Bytes>,
    host: Host,
    port: Option<u16>,
    params: Params,
    headers: Params,
}

#[derive(Debug, Clone)]
enum Parts {
    Sip(Box<SipParts>),
    /// Everything after the scheme, for schemes sipx does not model.
    Opaque(Bytes),
}

/// Borrowed syntax parts of an RFC 3966 `tel:` URI.
///
/// The view is deliberately byte-oriented and lossless. It neither removes visual separators
/// nor interprets parameters such as `phone-context`; those are policy decisions for the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelUriParts<'a> {
    subscriber: &'a [u8],
    parameters: Option<&'a [u8]>,
}

impl<'a> TelUriParts<'a> {
    /// The exact `telephone-subscriber` bytes before the first `;`.
    #[must_use]
    pub fn subscriber(&self) -> &'a [u8] {
        self.subscriber
    }

    /// The exact parameter tail after the first `;`, without that delimiter.
    ///
    /// `None` means there was no delimiter. `Some(b"")` retains a trailing delimiter from an
    /// opaque URI body, even though an empty parameter is not valid RFC 3966 syntax.
    #[must_use]
    pub fn parameters(&self) -> Option<&'a [u8]> {
        self.parameters
    }

    /// Iterate the exact generic RFC 3966 parameters with structural validation.
    ///
    /// Names compare case-insensitively through [`TelParameter::name_eq`]. Order, duplicates,
    /// percent escapes and original spelling are retained; parameter-specific policy is not
    /// applied. A malformed item is yielded once and fuses the iterator.
    #[must_use]
    pub fn parsed_parameters(&self) -> TelParameters<'a> {
        TelParameters {
            remaining: self.parameters,
            tail_len: self.parameters.map_or(0, <[u8]>::len),
        }
    }
}

/// Allocation-free iterator over one TEL URI's retained parameter tail.
#[derive(Debug, Clone)]
pub struct TelParameters<'a> {
    remaining: Option<&'a [u8]>,
    tail_len: usize,
}

/// One structurally valid generic RFC 3966 parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TelParameter<'a> {
    name: &'a [u8],
    value: Option<&'a [u8]>,
}

impl<'a> TelParameter<'a> {
    /// The exact parameter-name bytes.
    #[must_use]
    pub fn name(&self) -> &'a [u8] {
        self.name
    }

    /// The exact parameter value, or `None` when the wire parameter had no `=` delimiter.
    #[must_use]
    pub fn value(&self) -> Option<&'a [u8]> {
        self.value
    }

    /// Compare a valid RFC 3966 parameter name with ASCII case folding.
    ///
    /// An empty or syntactically invalid candidate is never equal.
    #[must_use]
    pub fn name_eq(&self, expected: &[u8]) -> bool {
        valid_tel_parameter_name(expected) && escape::eq_ignore_ascii_case(self.name, expected)
    }
}

/// Why one retained TEL parameter tail is not structurally valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("invalid TEL parameter at tail byte {offset}: {kind}")]
pub struct TelParameterError {
    offset: usize,
    kind: TelParameterErrorKind,
}

impl TelParameterError {
    /// Tail-relative byte offset of the offending component or byte.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// The rejected grammar component.
    #[must_use]
    pub fn kind(&self) -> TelParameterErrorKind {
        self.kind
    }
}

/// The malformed part of a generic RFC 3966 parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TelParameterErrorKind {
    /// An empty segment, including a trailing or repeated `;` delimiter.
    #[error("empty parameter")]
    Empty,
    /// An empty or syntactically invalid `pname`.
    #[error("invalid parameter name")]
    Name,
    /// An empty or syntactically invalid `pvalue`.
    #[error("invalid parameter value")]
    Value,
}

impl<'a> Iterator for TelParameters<'a> {
    type Item = Result<TelParameter<'a>, TelParameterError>;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.remaining.take()?;
        let offset = self.tail_len.saturating_sub(remaining.len());
        let (segment, rest) = match remaining.iter().position(|&byte| byte == b';') {
            Some(separator) => (
                remaining.get(..separator).unwrap_or(&[]),
                remaining.get(separator.saturating_add(1)..),
            ),
            None => (remaining, None),
        };
        self.remaining = rest;

        match parse_tel_parameter(segment, offset) {
            Ok(parameter) => Some(Ok(parameter)),
            Err(error) => {
                self.remaining = None;
                Some(Err(error))
            }
        }
    }
}

impl std::iter::FusedIterator for TelParameters<'_> {}

/// A URI.
///
/// # Equality
///
/// [`Uri`] deliberately does **not** implement `PartialEq` as RFC 3261 equivalence. That
/// relation is not transitive — the RFC says so in §19.1.4, and gives the example that
/// `sip:carol@chicago.com` is equivalent to both `sip:carol@chicago.com;security=on` and
/// `sip:carol@chicago.com;security=off`, which are not equivalent to each other. A
/// non-transitive `PartialEq` breaks `HashMap`, sorting, and every reader's assumptions.
///
/// So: [`Uri::equivalent`] implements the RFC relation and is what protocol logic must use.
#[derive(Debug, Clone)]
pub struct Uri {
    scheme: Scheme,
    parts: Parts,
    /// The exact wire form this URI last retained, so an untouched or span-rewritten URI is
    /// emitted without disturbing unrelated spelling. `None` for a constructed URI or after a
    /// general structured mutation.
    raw: Option<Bytes>,
    /// Exact span of an RFC 3966 telephone-subscriber in [`Self::raw`]. Present only for a
    /// parsed `tel:` URI, whose opaque body otherwise deliberately stays unmodelled.
    raw_tel_subscriber_span: Option<std::ops::Range<usize>>,
}

impl Uri {
    /// Parse a URI.
    ///
    /// The input must be the URI alone: any enclosing `<>` and surrounding whitespace belong
    /// to the header grammar and must be stripped by the caller.
    pub fn parse(raw: Bytes) -> Result<Self, UriError> {
        for &b in &raw {
            // The URI grammar is printable US-ASCII. Rejecting whitespace here is what makes
            // RFC 4475 3.1.2.8 (embedded LWS in a Request-URI) a parse failure rather than a
            // silently truncated host.
            if !(0x21..=0x7e).contains(&b) || matches!(b, b'<' | b'>' | b'"') {
                return Err(UriError::IllegalCharacter);
            }
        }

        let colon = raw
            .iter()
            .position(|&b| b == b':')
            .ok_or(UriError::Scheme)?;
        let scheme_raw = raw.slice(..colon);
        if scheme_raw.is_empty() || !scheme_raw.iter().all(|&b| is_scheme_char(b)) {
            return Err(UriError::Scheme);
        }
        let scheme = Scheme::parse(&scheme_raw);
        let rest = raw.slice(colon + 1..);
        if !escape::escapes_are_well_formed(&rest) {
            return Err(UriError::PercentEscape);
        }

        let raw_tel_subscriber_span = if matches!(scheme, Scheme::Tel) {
            let body_offset = colon.checked_add(1).ok_or(UriError::TelephoneSubscriber)?;
            let subscriber = split_tel_body(&rest).subscriber;
            validate_tel_subscriber(subscriber)?;
            let subscriber_len = subscriber.len();
            let end = body_offset
                .checked_add(subscriber_len)
                .ok_or(UriError::TelephoneSubscriber)?;
            Some(body_offset..end)
        } else {
            None
        };

        let parts = if scheme.is_sip() {
            Parts::Sip(Box::new(parse_sip_parts(&rest, colon + 1)?))
        } else {
            Parts::Opaque(rest)
        };

        Ok(Self {
            scheme,
            parts,
            raw: Some(raw),
            raw_tel_subscriber_span,
        })
    }

    /// Build a `sip:` or `sips:` URI.
    #[must_use]
    pub fn sip(host: Host) -> Self {
        Self {
            scheme: Scheme::Sip,
            parts: Parts::Sip(Box::new(SipParts {
                user: None,
                raw_user_span: None,
                password: None,
                host,
                port: None,
                params: Params::new(),
                headers: Params::new(),
            })),
            raw: None,
            raw_tel_subscriber_span: None,
        }
    }

    /// The scheme.
    #[must_use]
    pub fn scheme(&self) -> &Scheme {
        &self.scheme
    }

    #[must_use]
    fn sip_parts(&self) -> Option<&SipParts> {
        match &self.parts {
            Parts::Sip(p) => Some(p),
            Parts::Opaque(_) => None,
        }
    }

    #[must_use]
    fn sip_parts_mut(&mut self) -> Option<&mut SipParts> {
        match &mut self.parts {
            Parts::Sip(p) => {
                // Any general structured mutation invalidates the verbatim form and therefore
                // its retained user span. `replace_user` is the one operation that can update
                // both losslessly, so it does not enter through this helper.
                self.raw = None;
                p.raw_user_span = None;
                Some(p)
            }
            Parts::Opaque(_) => None,
        }
    }

    /// The user part, still percent-encoded, or `None` for a URI with no userinfo or a
    /// scheme sipx does not model.
    #[must_use]
    pub fn user(&self) -> Option<&[u8]> {
        self.sip_parts().and_then(|p| p.user.as_deref())
    }

    /// The password, still percent-encoded.
    ///
    /// Present in the grammar and therefore parsed; RFC 3261 §19.1.1 advises against using
    /// it, and sipx never puts one in a URI it builds.
    #[must_use]
    pub fn password(&self) -> Option<&[u8]> {
        self.sip_parts().and_then(|p| p.password.as_deref())
    }

    /// The user part with its percent escapes decoded.
    ///
    /// Yields bytes, not a string, and that is not an oversight: RFC 4475 §3.1.1.4 has a
    /// registration whose user part is `null-%00-null`, and `sip:%C3%A9@host` decodes to
    /// non-ASCII. Either would have to panic or be lossily replaced to become a `str`.
    ///
    /// Returns `None` if there is no user part or an escape is malformed.
    #[must_use]
    pub fn decoded_user(&self) -> Option<Vec<u8>> {
        self.user().and_then(escape::decode)
    }

    /// Replace an existing, already percent-encoded user part of a SIP or SIPS URI.
    ///
    /// Returns `Ok(false)` without touching the URI when its scheme is not SIP or SIPS or it has
    /// no user part. For a parsed URI, a valid replacement changes only the retained user span:
    /// scheme spelling, password, host spelling, delimiters, port, parameters and URI headers stay
    /// byte-identical. The old verbatim form is invalidated rather than replayed stale. A URI whose
    /// verbatim form was already discarded serializes canonically from its structured parts.
    ///
    /// # Errors
    ///
    /// [`UriError::PercentEscape`] reports a malformed `% HEX HEX` sequence. [`UriError::User`]
    /// reports an empty value or a byte outside RFC 3261 §25.1's `user` production. Either error
    /// leaves the URI unchanged.
    pub fn replace_user(&mut self, user: impl Into<Bytes>) -> Result<bool, UriError> {
        if !self.scheme.is_sip() {
            return Ok(false);
        }

        let (raw, parts) = (&mut self.raw, &mut self.parts);
        let Parts::Sip(parts) = parts else {
            return Ok(false);
        };
        if parts.user.is_none() {
            return Ok(false);
        }
        let user = user.into();
        validate_user(&user)?;

        let rewritten = match (raw.as_ref(), parts.raw_user_span.as_ref()) {
            (Some(verbatim), Some(span)) => {
                let end = span.start.checked_add(user.len()).ok_or(UriError::User)?;
                let value = replace_raw_span(verbatim, span, &user).ok_or(UriError::User)?;
                Some((value, span.start..end))
            }
            (Some(_), None) => return Err(UriError::User),
            (None, _) => None,
        };

        parts.user = Some(user);
        if let Some((value, span)) = rewritten {
            *raw = Some(value);
            parts.raw_user_span = Some(span);
        } else {
            *raw = None;
            parts.raw_user_span = None;
        }
        Ok(true)
    }

    /// Replace the telephone-subscriber of a parsed RFC 3966 `tel:` URI.
    ///
    /// Returns `Ok(false)` without touching the URI for every other scheme. A successful
    /// replacement splices only the parser-retained subscriber span, so mixed-case scheme
    /// spelling and the complete optional parameter tail stay byte-identical. This validates
    /// the global/local subscriber production but deliberately does not interpret parameters
    /// such as `phone-context`.
    ///
    /// # Errors
    ///
    /// [`UriError::TelephoneSubscriber`] reports an empty value or one outside RFC 3966's
    /// `global-number-digits` and `local-number-digits` productions. The error is atomic.
    pub fn replace_tel_subscriber(
        &mut self,
        subscriber: impl Into<Bytes>,
    ) -> Result<bool, UriError> {
        if !matches!(self.scheme, Scheme::Tel) {
            return Ok(false);
        }

        let subscriber = subscriber.into();
        validate_tel_subscriber(&subscriber)?;

        let (raw, parts, span) = (
            &mut self.raw,
            &mut self.parts,
            &mut self.raw_tel_subscriber_span,
        );
        let Parts::Opaque(body) = parts else {
            return Err(UriError::TelephoneSubscriber);
        };
        let (Some(verbatim), Some(current_span)) = (raw.as_ref(), span.as_ref()) else {
            return Err(UriError::TelephoneSubscriber);
        };
        let start = current_span.start;
        let end = start
            .checked_add(subscriber.len())
            .ok_or(UriError::TelephoneSubscriber)?;
        let rewritten = replace_raw_span(verbatim, current_span, &subscriber)
            .ok_or(UriError::TelephoneSubscriber)?;
        if rewritten.get(start..).is_none() {
            return Err(UriError::TelephoneSubscriber);
        }
        let mut rewritten_body = rewritten.clone();
        let rewritten_body = rewritten_body.split_off(start);

        *body = rewritten_body;
        *raw = Some(rewritten);
        *span = Some(start..end);
        Ok(true)
    }

    /// The host.
    #[must_use]
    pub fn host(&self) -> Option<&Host> {
        self.sip_parts().map(|p| &p.host)
    }

    /// The port, if the URI states one.
    ///
    /// A URI without a port is not the same as one naming the default port; see
    /// [`Uri::equivalent`].
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.sip_parts().and_then(|p| p.port)
    }

    /// The URI parameters — the `;name=value` list.
    #[must_use]
    pub fn params(&self) -> Option<&Params> {
        self.sip_parts().map(|p| &p.params)
    }

    /// The URI headers — the `?name=value&…` list.
    #[must_use]
    pub fn headers(&self) -> Option<&Params> {
        self.sip_parts().map(|p| &p.headers)
    }

    /// Everything after the scheme, for a scheme sipx does not model.
    #[must_use]
    pub fn opaque(&self) -> Option<&[u8]> {
        match &self.parts {
            Parts::Opaque(body) => Some(body),
            Parts::Sip(_) => None,
        }
    }

    /// Split an RFC 3966 `tel:` URI into exact subscriber and parameter-tail spans.
    ///
    /// Returns `None` for every other scheme. This is a syntax view only: it preserves visual
    /// separators, parameter spelling and order and performs no normalization or validation.
    #[must_use]
    pub fn tel_parts(&self) -> Option<TelUriParts<'_>> {
        match (&self.scheme, &self.parts) {
            (Scheme::Tel, Parts::Opaque(body)) => Some(split_tel_body(body)),
            _ => None,
        }
    }

    /// The value of the `transport` parameter.
    #[must_use]
    pub fn transport(&self) -> Option<&[u8]> {
        self.params().and_then(|p| p.value("transport"))
    }

    /// Select the effective transport without resolving the URI's host.
    pub fn selected_transport(&self) -> Result<UriTransport, UriTransportError> {
        if !self.scheme.is_sip() {
            return Err(UriTransportError::NotSip);
        }
        let explicit = match self.transport().map(<[u8]>::to_ascii_lowercase) {
            None => None,
            Some(value) => Some(match value.as_slice() {
                b"udp" => UriTransport::Udp,
                b"tcp" => UriTransport::Tcp,
                b"tls" => UriTransport::Tls,
                b"ws" => UriTransport::Ws,
                b"wss" => UriTransport::Wss,
                b"quic" => UriTransport::Quic,
                _ => return Err(UriTransportError::Unsupported),
            }),
        };
        if !self.scheme.is_secure() {
            return Ok(explicit.unwrap_or(UriTransport::Udp));
        }
        match explicit {
            None | Some(UriTransport::Tcp | UriTransport::Tls) => Ok(UriTransport::Tls),
            Some(UriTransport::Ws | UriTransport::Wss) => Ok(UriTransport::Wss),
            Some(UriTransport::Quic) => Ok(UriTransport::Quic),
            Some(UriTransport::Udp) => Err(UriTransportError::SecureDatagram),
        }
    }

    /// Add a URI parameter.
    ///
    /// Appended, not replaced: RFC 3261 §19.1.1 forbids a repeated `uri-parameter`, so a caller
    /// re-setting one of its own parameters wants [`Uri::remove_param`] first — see [`Params`].
    pub fn push_param(&mut self, param: Param) {
        if let Some(parts) = self.sip_parts_mut() {
            parts.params.push(param);
        }
    }

    /// Add a URI header component and report whether this URI can carry one.
    ///
    /// SIP and SIPS URIs have a `?name=value` component. Opaque schemes, including `tel`, do
    /// not; returning `false` lets History-Info follow RFC 7044 §10.2 without pretending a
    /// reason was embedded in a URI whose grammar has nowhere to put it.
    pub fn push_header(&mut self, header: Param) -> bool {
        let Some(parts) = self.sip_parts_mut() else {
            return false;
        };
        parts.headers.push(header);
        true
    }

    /// Remove every URI header component with this name.
    pub fn remove_header(&mut self, name: &str) -> bool {
        self.sip_parts_mut()
            .is_some_and(|parts| parts.headers.remove(name))
    }

    /// Remove a URI parameter, and say whether one was there.
    ///
    /// Names match the way §19.1.4 compares them, so `%74ransport` is `transport`.
    pub fn remove_param(&mut self, name: &str) -> bool {
        self.sip_parts_mut()
            .is_some_and(|parts| parts.params.remove(name))
    }

    /// Whether this URI carries any header components.
    ///
    /// A Request-URI must not (RFC 3261 §19.1.1); validation uses this.
    #[must_use]
    pub fn has_headers(&self) -> bool {
        self.headers().is_some_and(|h| !h.is_empty())
    }

    /// Serialize.
    ///
    /// A parsed, unmodified URI is written back exactly as it arrived.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        if let Some(raw) = &self.raw {
            out.extend_from_slice(raw);
            return;
        }
        out.extend_from_slice(self.scheme.as_bytes());
        out.push(b':');
        match &self.parts {
            Parts::Opaque(body) => out.extend_from_slice(body),
            Parts::Sip(p) => {
                if let Some(user) = &p.user {
                    out.extend_from_slice(user);
                    if let Some(password) = &p.password {
                        out.push(b':');
                        out.extend_from_slice(password);
                    }
                    out.push(b'@');
                }
                match &p.host {
                    Host::Ip(IpAddr::V6(ip)) => {
                        out.push(b'[');
                        out.extend_from_slice(ip.to_string().as_bytes());
                        out.push(b']');
                    }
                    host => out.extend_from_slice(&host.to_bytes()),
                }
                if let Some(port) = p.port {
                    out.push(b':');
                    out.extend_from_slice(port.to_string().as_bytes());
                }
                p.params.write_to(out, b';');
                p.headers.write_to(out, b'?');
            }
        }
    }

    /// Serialize to bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Bytes {
        let mut out = Vec::new();
        self.write_to(&mut out);
        Bytes::from(out)
    }

    /// Whether two URIs are equivalent under RFC 3261 §19.1.4.
    ///
    /// Note that this relation is **not transitive**; see the type-level documentation.
    #[must_use]
    pub fn equivalent(&self, other: &Self) -> bool {
        // "A SIP and SIPS URI are never equivalent."
        if !self.scheme.equivalent(&other.scheme) {
            return false;
        }

        let (a, b) = match (&self.parts, &other.parts) {
            (Parts::Sip(a), Parts::Sip(b)) => (a, b),
            (Parts::Opaque(a), Parts::Opaque(b)) => {
                // A tel URI has its own equivalence rules (RFC 3966 §4.1); byte comparison
                // would call `tel:+1-201-555-0123` and `tel:+12015550123` different numbers.
                if matches!(self.scheme, Scheme::Tel) {
                    return tel_equivalent(a, b);
                }
                // For schemes sipx does not model, no RFC defines comparison rules, so fall
                // back to the one thing that cannot be wrong: the bytes, after normalizing
                // escapes of unreserved characters.
                return escape::normalize_for_comparison(a) == escape::normalize_for_comparison(b);
            }
            _ => return false,
        };

        // "Comparison of the userinfo ... is case-sensitive", but escapes of unreserved
        // characters still fold: sip:%61lice@atlanta.com is sip:alice@atlanta.com.
        if !opt_bytes_equivalent(a.user.as_deref(), b.user.as_deref(), true) {
            return false;
        }
        if !opt_bytes_equivalent(a.password.as_deref(), b.password.as_deref(), true) {
            return false;
        }
        if !a.host.equivalent(&b.host) {
            return false;
        }
        // "A URI omitting any component with a default value will not match a URI explicitly
        // containing that component with its default value."
        if a.port != b.port {
            return false;
        }

        // "A user, ttl, or method uri-parameter appearing in only one URI never matches",
        // likewise maddr, and likewise transport per the paragraph above the list.
        for name in ["user", "ttl", "method", "maddr", "transport"] {
            if !a.params.param_equivalent(&b.params, name) {
                return false;
            }
        }
        // "Any uri-parameter appearing in both URIs must match." Others are ignored.
        if !a.params.common_params_agree(&b.params) || !b.params.common_params_agree(&a.params) {
            return false;
        }

        // "URI header components are never ignored. Any present header component MUST be
        // present in both URIs and match."
        //
        // Compared as multisets rather than by looking each one up by name. A URI may carry
        // the same header name twice with different values — `?f=a&f=b` is legal — and a
        // lookup returns only the first, so every occurrence after the first would be compared
        // against the wrong value. That made such a URI unequal to *itself*, which a property
        // test caught and no example test would have.
        header_multiset(&a.headers) == header_multiset(&b.headers)
    }
}

impl fmt::Display for Uri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.to_bytes()))
    }
}

/// The headers of a URI, grouped by name, with each name's values sorted.
///
/// Sorted because order carries no meaning for equivalence: `?a=1&b=2` and `?b=2&a=1` are the
/// same URI. Grouped because a name may repeat.
fn header_multiset(headers: &Params) -> std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>> {
    let mut grouped: std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>> =
        std::collections::BTreeMap::new();
    for param in headers.iter() {
        let name = escape::normalize_for_comparison(param.name()).to_ascii_lowercase();
        let value = param
            .value()
            .map(|value| escape::normalize_for_comparison(value).to_ascii_lowercase())
            .unwrap_or_default();
        grouped.entry(name).or_default().push(value);
    }
    for values in grouped.values_mut() {
        values.sort_unstable();
    }
    grouped
}

/// Whether two tel URI bodies — everything after `tel:` — are equivalent under RFC 3966 §4.1.
///
/// The number is compared after removing visual separators; both URIs must be global or both
/// local, which falls out of the comparison because only a global number keeps its leading
/// `+`. Parameters are compared by name regardless of order, a name present in only one URI
/// is a difference, and the whole comparison is case-insensitive.
#[must_use]
fn tel_equivalent(a: &[u8], b: &[u8]) -> bool {
    let parts_a = split_tel_body(a);
    let parts_b = split_tel_body(b);

    if !escape::eq_ignore_ascii_case(
        &strip_visual_separators(parts_a.subscriber),
        &strip_visual_separators(parts_b.subscriber),
    ) {
        return false;
    }

    tel_param_multiset(parts_a.parameters.unwrap_or_default())
        == tel_param_multiset(parts_b.parameters.unwrap_or_default())
}

/// Split a tel URI body into the telephone-subscriber part and the parameter tail.
#[must_use]
fn split_tel_body(body: &[u8]) -> TelUriParts<'_> {
    match body.iter().position(|&b| b == b';') {
        Some(semi) => TelUriParts {
            subscriber: body.get(..semi).unwrap_or(&[]),
            parameters: Some(body.get(semi + 1..).unwrap_or(&[])),
        },
        None => TelUriParts {
            subscriber: body,
            parameters: None,
        },
    }
}

fn parse_tel_parameter(
    segment: &[u8],
    offset: usize,
) -> Result<TelParameter<'_>, TelParameterError> {
    if segment.is_empty() {
        return Err(TelParameterError {
            offset,
            kind: TelParameterErrorKind::Empty,
        });
    }
    let (name, value, value_offset) = match segment.iter().position(|&byte| byte == b'=') {
        Some(equals) => (
            segment.get(..equals).unwrap_or(&[]),
            Some(segment.get(equals.saturating_add(1)..).unwrap_or(&[])),
            equals.saturating_add(1),
        ),
        None => (segment, None, segment.len()),
    };
    if name.is_empty() {
        return Err(TelParameterError {
            offset,
            kind: TelParameterErrorKind::Name,
        });
    }
    if let Some(invalid) = name
        .iter()
        .position(|&byte| !is_tel_parameter_name_char(byte))
    {
        return Err(TelParameterError {
            offset: offset.checked_add(invalid).unwrap_or(offset),
            kind: TelParameterErrorKind::Name,
        });
    }
    if let Some(value) = value {
        if value.is_empty() {
            return Err(TelParameterError {
                offset: offset.checked_add(value_offset).unwrap_or(offset),
                kind: TelParameterErrorKind::Value,
            });
        }
        if let Some(invalid) = invalid_tel_parameter_value_byte(value) {
            let value_start = offset.checked_add(value_offset).unwrap_or(offset);
            return Err(TelParameterError {
                offset: value_start.checked_add(invalid).unwrap_or(value_start),
                kind: TelParameterErrorKind::Value,
            });
        }
    }
    Ok(TelParameter { name, value })
}

#[must_use]
fn valid_tel_parameter_name(name: &[u8]) -> bool {
    !name.is_empty() && name.iter().copied().all(is_tel_parameter_name_char)
}

#[must_use]
fn is_tel_parameter_name_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

#[must_use]
fn invalid_tel_parameter_value_byte(value: &[u8]) -> Option<usize> {
    let mut index = 0;
    while let Some(&byte) = value.get(index) {
        if byte == b'%' {
            let first = value.get(index.saturating_add(1));
            let second = value.get(index.saturating_add(2));
            if !first.is_some_and(u8::is_ascii_hexdigit)
                || !second.is_some_and(u8::is_ascii_hexdigit)
            {
                return Some(index);
            }
            index = index.saturating_add(3);
            continue;
        }
        if !(byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'.'
                    | b'_'
                    | b'!'
                    | b'~'
                    | b'*'
                    | b'\''
                    | b'('
                    | b')'
                    | b'['
                    | b']'
                    | b'/'
                    | b':'
                    | b'&'
                    | b'+'
                    | b'$'
            ))
        {
            return Some(index);
        }
        index = index.saturating_add(1);
    }
    None
}

/// Remove the RFC 3966 `visual-separator` characters: `-`, `.`, `(` and `)`.
#[must_use]
fn strip_visual_separators(number: &[u8]) -> Vec<u8> {
    number
        .iter()
        .copied()
        .filter(|b| !matches!(b, b'-' | b'.' | b'(' | b')'))
        .collect()
}

/// The parameters of a tel URI, grouped by name, normalized for the §4.1 comparison.
///
/// Escapes of unreserved characters fold, and everything lowercases — "URI comparisons are
/// case-insensitive". A `phone-context` naming a global number, and an `ext`, are digit
/// strings, so their visual separators are removed the same way the number's are.
fn tel_param_multiset(params: &[u8]) -> std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>> {
    let mut grouped: std::collections::BTreeMap<Vec<u8>, Vec<Vec<u8>>> =
        std::collections::BTreeMap::new();
    if params.is_empty() {
        return grouped;
    }
    for segment in params.split(|&b| b == b';') {
        let (name, value) = match segment.iter().position(|&b| b == b'=') {
            Some(eq) => (
                segment.get(..eq).unwrap_or(&[]),
                segment.get(eq + 1..).unwrap_or(&[]),
            ),
            None => (segment, &[][..]),
        };
        let name = escape::normalize_for_comparison(name).to_ascii_lowercase();
        let mut value = escape::normalize_for_comparison(value).to_ascii_lowercase();
        if name == b"ext" || (name == b"phone-context" && value.first() == Some(&b'+')) {
            value = strip_visual_separators(&value);
        }
        grouped.entry(name).or_default().push(value);
    }
    for values in grouped.values_mut() {
        values.sort_unstable();
    }
    grouped
}

/// Compare two optional components, folding escapes of unreserved characters. `case_sensitive`
/// distinguishes userinfo (case-sensitive) from everything else.
#[must_use]
fn opt_bytes_equivalent(a: Option<&[u8]>, b: Option<&[u8]>, case_sensitive: bool) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            let (x, y) = (
                escape::normalize_for_comparison(x),
                escape::normalize_for_comparison(y),
            );
            if case_sensitive {
                x == y
            } else {
                escape::eq_ignore_ascii_case(&x, &y)
            }
        }
        _ => false,
    }
}

#[must_use]
fn is_scheme_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')
}

/// Validate RFC 3261 §25.1's already percent-encoded `user` production.
fn validate_user(user: &[u8]) -> Result<(), UriError> {
    if !escape::escapes_are_well_formed(user) {
        return Err(UriError::PercentEscape);
    }
    if user.is_empty() || !user.iter().copied().all(is_user_char) {
        return Err(UriError::User);
    }
    Ok(())
}

/// Validate RFC 3966 §3's `global-number-digits / local-number-digits` production.
fn validate_tel_subscriber(subscriber: &[u8]) -> Result<(), UriError> {
    let valid = if let Some(rest) = subscriber.strip_prefix(b"+") {
        rest.iter().copied().all(is_global_phone_digit) && rest.iter().any(u8::is_ascii_digit)
    } else {
        subscriber.iter().copied().all(is_local_phone_digit)
            && subscriber.iter().copied().any(is_local_phone_symbol)
    };
    if valid {
        Ok(())
    } else {
        Err(UriError::TelephoneSubscriber)
    }
}

#[must_use]
fn is_global_phone_digit(byte: u8) -> bool {
    byte.is_ascii_digit() || is_visual_separator(byte)
}

#[must_use]
fn is_local_phone_digit(byte: u8) -> bool {
    is_local_phone_symbol(byte) || is_visual_separator(byte)
}

#[must_use]
fn is_local_phone_symbol(byte: u8) -> bool {
    byte.is_ascii_hexdigit() || matches!(byte, b'*' | b'#')
}

#[must_use]
fn is_visual_separator(byte: u8) -> bool {
    matches!(byte, b'-' | b'.' | b'(' | b')')
}

#[must_use]
fn is_user_char(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'-' | b'_'
                | b'.'
                | b'!'
                | b'~'
                | b'*'
                | b'\''
                | b'('
                | b')'
                | b'&'
                | b'='
                | b'+'
                | b'$'
                | b','
                | b';'
                | b'?'
                | b'/'
                | b'%'
        )
}

/// Hostname characters.
///
/// The ABNF permits only alphanumerics, `-` and `.`. sipx also accepts `_`, which the grammar
/// does not: it is common in deployed hostnames, and it is not a delimiter anywhere in the
/// URI grammar, so accepting it cannot make a URI ambiguous.
#[must_use]
fn is_host_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_')
}

fn parse_sip_parts(rest: &Bytes, body_offset: usize) -> Result<SipParts, UriError> {
    // An unescaped '@' can only be the userinfo separator: it is absent from the character
    // sets for user, password, host, parameters and headers alike, so it must be escaped
    // anywhere else. That makes the first '@' unambiguous.
    let (userinfo, after) = match rest.iter().position(|&b| b == b'@') {
        Some(at) => (Some(rest.slice(..at)), rest.slice(at + 1..)),
        None => (None, rest.clone()),
    };

    let (user, password) = match userinfo {
        None => (None, None),
        Some(info) => match info.iter().position(|&b| b == b':') {
            // The password may not contain ':', so the first one separates the two.
            Some(c) => (Some(info.slice(..c)), Some(info.slice(c + 1..))),
            None => (Some(info), None),
        },
    };
    if let Some(user) = &user {
        validate_user(user)?;
    }
    let raw_user_span = user
        .as_ref()
        .and_then(|value| body_offset.checked_add(value.len()))
        .map(|end| body_offset..end);

    for field in [user.as_ref(), password.as_ref()].into_iter().flatten() {
        if !escape::escapes_are_well_formed(field) {
            return Err(UriError::PercentEscape);
        }
    }

    // Headers start at the first '?'; parameters at the first ';'. Neither character can
    // appear unescaped in a host, so scanning left to right is unambiguous here — which is
    // only true because userinfo has already been removed. The user part *may* contain both.
    let (before_headers, headers_raw) = match after.iter().position(|&b| b == b'?') {
        Some(q) => (after.slice(..q), Some(after.slice(q + 1..))),
        None => (after.clone(), None),
    };
    let (hostport, params_raw) = match before_headers.iter().position(|&b| b == b';') {
        Some(s) => (
            before_headers.slice(..s),
            Some(before_headers.slice(s + 1..)),
        ),
        None => (before_headers.clone(), None),
    };

    let (host, port) = parse_hostport(&hostport)?;

    let params = match params_raw {
        Some(raw) => parse_params(&raw, b';')?,
        None => Params::new(),
    };
    let headers = match headers_raw {
        Some(raw) => parse_params(&raw, b'&')?,
        None => Params::new(),
    };

    Ok(SipParts {
        user,
        raw_user_span,
        password,
        host,
        port,
        params,
        headers,
    })
}

/// Replace a parser-owned span without re-reading URI grammar.
fn replace_raw_span(
    raw: &Bytes,
    span: &std::ops::Range<usize>,
    replacement: &[u8],
) -> Option<Bytes> {
    let before = raw.get(..span.start)?;
    let after = raw.get(span.end..)?;
    let capacity = before
        .len()
        .checked_add(replacement.len())?
        .checked_add(after.len())?;
    let mut rewritten = Vec::with_capacity(capacity);
    rewritten.extend_from_slice(before);
    rewritten.extend_from_slice(replacement);
    rewritten.extend_from_slice(after);
    Some(Bytes::from(rewritten))
}

fn parse_hostport(hostport: &Bytes) -> Result<(Host, Option<u16>), UriError> {
    if hostport.is_empty() {
        return Err(UriError::EmptyHost);
    }

    if hostport.first() == Some(&b'[') {
        let close = hostport
            .iter()
            .position(|&b| b == b']')
            .ok_or(UriError::Ipv6Reference)?;
        let inner = hostport.slice(1..close);
        let text = std::str::from_utf8(&inner).map_err(|_| UriError::Host)?;
        let ip = parse_ipv6_reference(text)?;
        let tail = hostport.slice(close + 1..);
        let port = parse_port_suffix(&tail)?;
        return Ok((Host::Ip(IpAddr::V6(ip)), port));
    }

    let (host_raw, port) = match hostport.iter().position(|&b| b == b':') {
        Some(c) => (hostport.slice(..c), parse_port(&hostport.slice(c + 1..))?),
        None => (hostport.clone(), None),
    };

    if host_raw.is_empty() {
        return Err(UriError::EmptyHost);
    }
    if !host_raw.iter().all(|&b| is_host_char(b)) {
        return Err(UriError::Host);
    }

    let host = std::str::from_utf8(&host_raw)
        .ok()
        .and_then(|s| s.parse::<Ipv4Addr>().ok())
        .map_or_else(
            || Host::Name(HostName::new_unchecked(host_raw.clone())),
            |ip| Host::Ip(IpAddr::V4(ip)),
        );

    Ok((host, port))
}

/// Parse the text between an IPv6 reference's `[` and `]`.
///
/// RFC 4291 §2.2 is the address grammar, and `Ipv6Addr`'s own parser implements it. Almost
/// everything goes through that parser untouched; this function exists for the one construct
/// RFC 4291 forbids and sipx must accept anyway.
///
/// RFC 3261 §25.1 inherited its `IPv6address` production from the obsoleted RFC 2373:
///
/// ```abnf
/// IPv6address = hexpart [ ":" IPv4address ]
/// hexpart     = hexseq / hexseq "::" [ hexseq ] / "::" [ hexseq ]
/// ```
///
/// `hexpart` may end in `"::"`, and the grammar then appends `":" IPv4address` — so RFC 3261's
/// own ABNF derives `2001:db8:::192.0.2.1`, with three colons before an embedded IPv4 address.
/// RFC 4291 corrected the grammar, but senders had already been written against RFC 3261, and
/// RFC 5118 §4.10 is normative about the consequence: "following the Robustness Principle
/// [RFC1122], an implementation must tolerate both of the above constructs."
///
/// # The rule, and why it is this narrow
///
/// `:::` reads as `::` **only** immediately before an embedded IPv4 address that ends the
/// reference — the one position the derivation above can produce it. Everywhere else `:::` stays
/// `UriError::Host`. See `docs/specs/sip-parser.md` §4.8.
///
/// The tolerance is a rewrite of one `:::` into `::` followed by a **retry through the same
/// RFC 4291 parser**, never a parser of its own. So the language accepted is exactly RFC 4291
/// plus that single derivation: `2001:db8::1:::192.0.2.1` rewrites to a reference with two `::`
/// runs and is still rejected, `[2001:db8:::10]` has no embedded IPv4 address and is still
/// rejected, and `::::192.0.2.1` leaves a leading colon on the tail and is still rejected.
/// Widening the address grammar instead would trade one unmet MUST for an unmeasured surface on
/// unauthenticated input.
fn parse_ipv6_reference(text: &str) -> Result<Ipv6Addr, UriError> {
    if let Ok(ip) = text.parse::<Ipv6Addr>() {
        return Ok(ip);
    }

    // RFC 5118 §4.10. `split_once` takes the *first* `:::`, which is what makes the check below
    // sufficient rather than merely indicative: a second `:::`, or a fourth colon, leaves a tail
    // that is not an `IPv4address`, and an embedded IPv4 address is by definition the end of the
    // reference. So there is no second occurrence to reason about separately.
    let (hexpart, embedded) = text.split_once(":::").ok_or(UriError::Host)?;
    if embedded.parse::<Ipv4Addr>().is_err() {
        return Err(UriError::Host);
    }

    let mut corrected = String::with_capacity(text.len());
    corrected.push_str(hexpart);
    corrected.push_str("::");
    corrected.push_str(embedded);
    corrected.parse::<Ipv6Addr>().map_err(|_| UriError::Host)
}

fn parse_port_suffix(tail: &Bytes) -> Result<Option<u16>, UriError> {
    if tail.is_empty() {
        return Ok(None);
    }
    if tail.first() != Some(&b':') {
        return Err(UriError::Host);
    }
    parse_port(&tail.slice(1..))
}

fn parse_port(raw: &Bytes) -> Result<Option<u16>, UriError> {
    if raw.is_empty() || !raw.iter().all(u8::is_ascii_digit) {
        return Err(UriError::Port);
    }
    // Explicitly bounded: a port is at most five digits, so a long run of digits is rejected
    // before any conversion rather than wrapping.
    if raw.len() > 5 {
        return Err(UriError::Port);
    }
    let mut value: u32 = 0;
    for &b in raw {
        value = value * 10 + u32::from(b - b'0');
    }
    u16::try_from(value).map(Some).map_err(|_| UriError::Port)
}

fn parse_params(raw: &Bytes, separator: u8) -> Result<Params, UriError> {
    let mut params = Params::new();
    // A trailing separator with nothing after it (`sip:host;`) is not a parameter list of
    // length zero; the ABNF's pname is `1*paramchar`, so there is nothing legal to parse.
    if raw.is_empty() {
        return Err(UriError::EmptyParameterName);
    }
    let mut start = 0usize;
    let cut = |from: usize, to: usize, params: &mut Params| -> Result<(), UriError> {
        let field = raw.slice(from..to);
        // Rejected rather than skipped: `;;` is not a quirky spelling of `;`. The header
        // grammar takes the same line, and RFC 4475 3.1.2.1 turns a message invalid on
        // exactly this in a Via.
        if field.is_empty() {
            return Err(UriError::EmptyParameterName);
        }
        let param = match field.iter().position(|&b| b == b'=') {
            Some(eq) => {
                let name = field.slice(..eq);
                if name.is_empty() {
                    return Err(UriError::EmptyParameterName);
                }
                Param::new(name, field.slice(eq + 1..))
            }
            None => Param::flag(field),
        };
        if !escape::escapes_are_well_formed(param.name())
            || param
                .value()
                .is_some_and(|v| !escape::escapes_are_well_formed(v))
        {
            return Err(UriError::PercentEscape);
        }
        // RFC 3261 §19.1.1: "any given parameter-name MUST NOT appear more than once" among
        // uri-parameters. URI headers may legally repeat — `?f=a&f=b` — so only the `;` list
        // is policed. Spelling variants count: §19.1.4 makes `%74ransport` the name
        // `transport`, so a repeat under an escape or a case change is still a repeat.
        if separator == b';'
            && params
                .iter()
                .any(|existing| crate::params::names_equivalent(existing.name(), param.name()))
        {
            return Err(UriError::DuplicateParameterName);
        }
        params.push(param);
        Ok(())
    };

    for (i, &b) in raw.iter().enumerate() {
        if b == separator {
            cut(start, i, &mut params)?;
            start = i + 1;
        }
    }
    cut(start, raw.len(), &mut params)?;
    Ok(params)
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

    fn uri(s: &str) -> Uri {
        Uri::parse(Bytes::from(s.to_owned())).unwrap_or_else(|e| panic!("{s:?} should parse: {e}"))
    }

    /// The worked examples from RFC 3261 §19.1.4. Every one of these fails under naive string
    /// comparison, which is the entire reason the section exists.
    #[test]
    fn uri_equivalence_rfc3261_19_1_4() {
        let equivalent: &[(&str, &str)] = &[
            (
                "sip:%61lice@atlanta.com;transport=TCP",
                "sip:alice@AtLanTa.CoM;Transport=tcp",
            ),
            ("sip:carol@chicago.com", "sip:carol@chicago.com;newparam=5"),
            ("sip:carol@chicago.com", "sip:carol@chicago.com;security=on"),
            (
                "sip:carol@chicago.com;newparam=5",
                "sip:carol@chicago.com;security=on",
            ),
            (
                "sip:biloxi.com;transport=tcp;method=REGISTER?to=sip:bob%40biloxi.com",
                "sip:biloxi.com;method=REGISTER;transport=tcp?to=sip:bob%40biloxi.com",
            ),
            (
                "sip:alice@atlanta.com?subject=project%20x&priority=urgent",
                "sip:alice@atlanta.com?priority=urgent&subject=project%20x",
            ),
        ];
        for (a, b) in equivalent {
            assert!(
                uri(a).equivalent(&uri(b)),
                "RFC 3261 19.1.4 says these are equivalent:\n  {a}\n  {b}"
            );
            assert!(uri(b).equivalent(&uri(a)), "equivalence must be symmetric");
        }

        let different: &[(&str, &str, &str)] = &[
            (
                "SIP:ALICE@AtLanTa.CoM;Transport=udp",
                "sip:alice@AtLanTa.CoM;Transport=UDP",
                "different usernames",
            ),
            (
                "sip:bob@biloxi.com",
                "sip:bob@biloxi.com:5060",
                "can resolve to different ports",
            ),
            (
                "sip:bob@biloxi.com",
                "sip:bob@biloxi.com;transport=udp",
                "can resolve to different transports",
            ),
            (
                "sip:bob@biloxi.com",
                "sip:bob@biloxi.com:6000;transport=tcp",
                "different port and transport",
            ),
            (
                "sip:carol@chicago.com",
                "sip:carol@chicago.com?Subject=next%20meeting",
                "different header component",
            ),
            (
                "sip:bob@phone21.boxesbybob.com",
                "sip:bob@192.0.2.4",
                "a hostname never matches an IP address",
            ),
        ];
        for (a, b, why) in different {
            assert!(
                !uri(a).equivalent(&uri(b)),
                "RFC 3261 19.1.4 says these differ ({why}):\n  {a}\n  {b}"
            );
        }
    }

    #[test]
    fn sip_and_sips_are_never_equivalent() {
        assert!(!uri("sip:a@b.com").equivalent(&uri("sips:a@b.com")));
    }

    /// The RFC 3966 §4.1 rules: visual separators are not part of the number, parameter
    /// order and case carry no meaning, and a parameter present in only one URI is a
    /// difference.
    #[test]
    fn tel_uri_equivalence_rfc3966_4_1() {
        let equivalent: &[(&str, &str)] = &[
            // The §4.1 worked example: separators removed, the numbers are identical.
            ("tel:+1-201-555-0123", "tel:+12015550123"),
            (
                "tel:7042;phone-context=example.com",
                "tel:7042;PHONE-CONTEXT=EXAMPLE.COM",
            ),
            // A global-number phone-context is compared digit by digit, separators removed.
            (
                "tel:863-1234;phone-context=+1-914-555",
                "tel:8631234;phone-context=+1914555",
            ),
            // Parameter order is insignificant.
            (
                "tel:7042;ext=1;phone-context=example.com",
                "tel:7042;phone-context=example.com;ext=1",
            ),
        ];
        for (a, b) in equivalent {
            assert!(
                uri(a).equivalent(&uri(b)),
                "RFC 3966 4.1 says these are equivalent:\n  {a}\n  {b}"
            );
            assert!(uri(b).equivalent(&uri(a)), "equivalence must be symmetric");
        }

        let different: &[(&str, &str, &str)] = &[
            (
                "tel:+12015550123",
                "tel:12015550123",
                "a global number never matches a local one",
            ),
            (
                "tel:7042;phone-context=example.com",
                "tel:7042",
                "a parameter present in only one is a difference",
            ),
            (
                "tel:+1-201-555-0123",
                "tel:+1-201-555-0124",
                "different numbers",
            ),
            (
                "tel:7042;phone-context=example.com",
                "tel:7042;phone-context=example.org",
                "different phone-context domains",
            ),
        ];
        for (a, b, why) in different {
            assert!(
                !uri(a).equivalent(&uri(b)),
                "RFC 3966 4.1 says these differ ({why}):\n  {a}\n  {b}"
            );
        }
    }

    /// RFC 3261 §19.1.4: characters outside the reserved set are equivalent to their
    /// `% HEX HEX` encoding, and `pname` is built from `paramchar`, which includes
    /// `escaped` — so `%74ransport` is a legal spelling of `transport`, and the §19.1.4
    /// special-parameter rules must see through it.
    #[test]
    fn escaped_parameter_names_are_the_same_parameter() {
        assert!(uri("sip:h;transport=udp").equivalent(&uri("sip:h;%74ransport=udp")));
        assert_eq!(uri("sip:h;%74ransport=tcp").transport(), Some(&b"tcp"[..]));

        // A one-sided maddr never matches, however its name is spelled.
        assert!(!uri("sip:h").equivalent(&uri("sip:h;m%61ddr=239.1.1.1")));
        assert!(!uri("sip:h;m%61ddr=239.1.1.1").equivalent(&uri("sip:h")));

        // And a non-special parameter present in both must still agree.
        assert!(!uri("sip:h;foo=1").equivalent(&uri("sip:h;%66oo=2")));
    }

    /// The RFC notes this itself: equivalence is not transitive. It is the reason `Uri` does
    /// not implement `PartialEq` as equivalence.
    #[test]
    fn equivalence_is_not_transitive() {
        let plain = uri("sip:carol@chicago.com");
        let on = uri("sip:carol@chicago.com;security=on");
        let off = uri("sip:carol@chicago.com;security=off");
        assert!(plain.equivalent(&on));
        assert!(plain.equivalent(&off));
        assert!(!on.equivalent(&off));
    }

    #[test]
    fn parses_userinfo_with_password() {
        let u = uri("sip:alice:secret@atlanta.com");
        assert_eq!(u.user(), Some(&b"alice"[..]));
        assert_eq!(u.password(), Some(&b"secret"[..]));
    }

    /// RFC 4475 3.1.1.2: the user part may contain `?`, `;` and `/`, so neither the
    /// parameter nor the header scan may run before userinfo has been removed.
    #[test]
    fn user_part_may_contain_parameter_and_header_delimiters() {
        let u = uri(
            "sip:1_unusual.URI~(to-be!sure)&isn't+it$/crazy?,/;;*:&it+has=1,weird!*pas$wo~d_too.(doesn't-it)@example.com",
        );
        assert_eq!(
            u.user(),
            Some(&b"1_unusual.URI~(to-be!sure)&isn't+it$/crazy?,/;;*"[..])
        );
        assert_eq!(
            u.password(),
            Some(&b"&it+has=1,weird!*pas$wo~d_too.(doesn't-it)"[..])
        );
        assert!(matches!(u.host(), Some(Host::Name(h)) if *h == "example.com"));
        assert!(u.params().is_some_and(Params::is_empty));
        assert!(!u.has_headers());
    }

    /// RFC 4475 3.1.1.9: semicolons in the user part are user-part characters, not parameter
    /// separators.
    #[test]
    fn semicolons_in_user_part_are_not_parameters() {
        let u = uri("sip:user;par=u%40example.net@example.com");
        assert_eq!(u.user(), Some(&b"user;par=u%40example.net"[..]));
        assert!(u.params().is_some_and(Params::is_empty));
    }

    /// RFC 4475 3.1.1.4: the user part is `null-%00-null`. Decoding must yield the NUL, which
    /// is why this returns bytes.
    #[test]
    fn decodes_escaped_null_in_user_part() {
        let u = uri("sip:null-%00-null@example.com");
        assert_eq!(u.user(), Some(&b"null-%00-null"[..]));
        assert_eq!(u.decoded_user(), Some(b"null-\x00-null".to_vec()));
        // The escaped form is what goes back on the wire.
        assert_eq!(
            u.to_bytes(),
            Bytes::from_static(b"sip:null-%00-null@example.com")
        );
    }

    #[test]
    fn parses_ipv6_reference_with_and_without_port() {
        let u = uri("sip:alice@[2001:db8::1]");
        assert!(matches!(u.host(), Some(Host::Ip(IpAddr::V6(_)))));
        assert_eq!(u.port(), None);

        let u = uri("sip:alice@[2001:db8::1]:5061");
        assert_eq!(u.port(), Some(5061));
    }

    /// RFC 5118 §4.10: the three-colon reference RFC 3261's ABNF derives must be tolerated, and
    /// must mean the address its two-colon twin means.
    #[test]
    fn tolerates_three_colons_before_an_embedded_ipv4_address() {
        let buggy = uri("sip:user@[2001:db8:::192.0.2.1]");
        let correct = uri("sip:user@[2001:db8::192.0.2.1]");
        let expected = "2001:db8::192.0.2.1"
            .parse::<IpAddr>()
            .expect("a valid RFC 4291 address");

        for (u, what) in [(&buggy, "three-colon"), (&correct, "two-colon")] {
            match u.host() {
                Some(Host::Ip(ip)) => assert_eq!(*ip, expected, "{what} form"),
                other => panic!("{what} form should be an IPv6 literal, got {other:?}"),
            }
        }

        // Tolerated, not normalised: the reference goes back on the wire as it arrived.
        assert_eq!(
            buggy.to_bytes(),
            Bytes::from_static(b"sip:user@[2001:db8:::192.0.2.1]")
        );

        // The tolerance reaches a `Via` sent-by too, because both go through `parse_hostport` —
        // RFC 3261's ABNF derives the construct wherever `IPv6reference` appears, so a rule that
        // held only in the Request-URI would be a second, narrower grammar nobody could cite.
        let (host, port) =
            Host::parse_hostport(&Bytes::from_static(b"[2001:db8:::192.0.2.1]:5060"))
                .expect("a Via sent-by holds an IPv6reference too");
        assert!(matches!(host, Host::Ip(ip) if ip == expected));
        assert_eq!(port, Some(5060));

        // RFC 2373's `hexpart` offers two productions that can end in "::", and both derive the
        // three-colon form. §4.10's own message exercises `hexseq "::"`; these are the other one
        // (empty `hexseq`) and the same one at full width. Covered here through `Uri::parse` as
        // well as in the spec-table pin, so the carve-out is known to work on the R-URI path.
        for (input, want) in [
            ("sip:user@[:::192.0.2.1]", "::192.0.2.1"),
            ("sip:user@[1:2:3:4:5:::192.0.2.1]", "1:2:3:4:5::192.0.2.1"),
        ] {
            let want = want.parse::<IpAddr>().expect("a valid RFC 4291 address");
            match uri(input).host() {
                Some(Host::Ip(ip)) => assert_eq!(*ip, want, "{input}"),
                other => panic!("{input} should be an IPv6 literal, got {other:?}"),
            }
        }
    }

    /// The narrowness is the story: `:::` is read as `::` in exactly one position, and every other
    /// place it can appear stays a typed error rather than an address parsed on a guess.
    #[test]
    fn three_colons_anywhere_but_before_an_embedded_ipv4_address_stay_rejected() {
        // The variant is asserted, not merely the failure. `is_err()` alone would have let the
        // last two rows pass while the spec named the wrong error for them, and the variant is
        // what the transaction layer picks a response code from.
        let rejected: &[(&str, UriError)] = &[
            // No embedded IPv4 address at all — the derivation cannot produce ':::' here.
            ("sip:user@[2001:db8:::10]", UriError::Host),
            ("sip:user@[2001:db8:::]", UriError::Host),
            ("sip:user@[:::]", UriError::Host),
            // ':::' before something that only looks like one.
            ("sip:user@[2001:db8:::192.0.2]", UriError::Host),
            ("sip:user@[2001:db8:::192.0.2.1.5]", UriError::Host),
            ("sip:user@[2001:db8:::192.0.2.256]", UriError::Host),
            ("sip:user@[2001:db8:::0192.0.2.1]", UriError::Host),
            // A fourth colon is not the derivation; it leaves a colon on the tail.
            ("sip:user@[2001:db8::::192.0.2.1]", UriError::Host),
            ("sip:user@[::::192.0.2.1]", UriError::Host),
            // Two occurrences, and a '::' run already spent — the rewrite must not create a
            // second one and have it accepted.
            (
                "sip:user@[2001:db8:::192.0.2.1:::192.0.2.2]",
                UriError::Host,
            ),
            ("sip:user@[2001:db8::1:::192.0.2.1]", UriError::Host),
            // ':::' in the middle rather than before the embedded address.
            ("sip:user@[2001:::db8:192.0.2.1]", UriError::Host),
            // Unbracketed, and it fails *before* any address parser sees it: the host is split at
            // its first ':' and `db8:::192.0.2.1` is rejected as a port. The valid two-colon
            // address below fails identically, which is the point — RFC 3261 §19.1.1's brackets
            // are what make an IPv6 address reachable at all, not the carve-out.
            ("sip:user@2001:db8:::192.0.2.1", UriError::Port),
            ("sip:user@2001:db8::192.0.2.1", UriError::Port),
        ];
        for (input, expected) in rejected {
            let got = Uri::parse(Bytes::from((*input).to_owned()));
            assert_eq!(
                got.as_ref().err(),
                Some(expected),
                "{input:?} is not RFC 5118 §4.10's construct and must stay {expected:?}, \
                 got {got:?}"
            );
        }
    }

    #[test]
    fn parses_ipv4_literal_as_an_address() {
        let u = uri("sip:bob@192.0.2.4");
        assert!(matches!(u.host(), Some(Host::Ip(IpAddr::V4(_)))));
    }

    #[test]
    fn scheme_and_parameter_select_one_fail_closed_transport_and_default_port() {
        let cases = [
            ("sip:h;transport=tcp", UriTransport::Tcp, 5060),
            ("sip:h;transport=ws", UriTransport::Ws, 80),
            ("sips:h;transport=tcp", UriTransport::Tls, 5061),
            ("sips:h;transport=tls", UriTransport::Tls, 5061),
            ("sips:h;transport=ws", UriTransport::Wss, 443),
            ("sips:h;transport=wss", UriTransport::Wss, 443),
        ];
        for (input, expected, port) in cases {
            let selected = uri(input).selected_transport().expect("supported mapping");
            assert_eq!(selected, expected, "{input}");
            assert_eq!(selected.default_port(), port, "{input}");
        }
        assert_eq!(
            uri("sips:h;transport=udp").selected_transport(),
            Err(UriTransportError::SecureDatagram)
        );
    }

    #[test]
    fn unknown_schemes_are_kept_opaque() {
        // RFC 4475 3.3.2: a Request-URI with an unknown scheme must parse; answering 416 is
        // the application's business.
        let u = uri("nobodyKnowsThisScheme:totally-bogus-stuff");
        assert!(matches!(u.scheme(), Scheme::Other(_)));
        assert_eq!(u.opaque(), Some(&b"totally-bogus-stuff"[..]));
        assert!(u.host().is_none());
    }

    #[test]
    fn rejects_malformed_uris() {
        let cases: &[(&str, UriError)] = &[
            ("sip:alice@example .com", UriError::IllegalCharacter),
            ("sip:alice@exa\tmple.com", UriError::IllegalCharacter),
            ("<sip:alice@example.com>", UriError::IllegalCharacter),
            ("alice@example.com", UriError::Scheme),
            (":alice@example.com", UriError::Scheme),
            ("sip:", UriError::EmptyHost),
            ("sip:alice@", UriError::EmptyHost),
            ("sip:alice@host:70000", UriError::Port),
            ("sip:alice@host:", UriError::Port),
            ("sip:alice@host:12x", UriError::Port),
            ("sip:alice@[2001:db8::1", UriError::Ipv6Reference),
            ("sip:alice%zz@host", UriError::PercentEscape),
        ];
        for (input, expected) in cases {
            let got = Uri::parse(Bytes::from((*input).to_owned()));
            assert_eq!(
                got.as_ref().err(),
                Some(expected),
                "{input:?} should be rejected as {expected:?}, got {got:?}"
            );
        }
    }

    #[test]
    fn port_five_digits_is_bounded_before_conversion() {
        // 99999 is five digits and still out of range; 999999 is rejected on length alone.
        assert!(Uri::parse(Bytes::from_static(b"sip:h:99999")).is_err());
        assert!(Uri::parse(Bytes::from_static(b"sip:h:999999")).is_err());
        assert_eq!(uri("sip:h:65535").port(), Some(65535));
    }

    #[test]
    fn a_parsed_uri_round_trips_byte_exactly() {
        for input in [
            "sip:vivekg@chair-dnrc.example.com;unknownparam",
            "SIP:ALICE@AtLanTa.CoM;Transport=udp",
            "sip:biloxi.com;transport=tcp;method=REGISTER?to=sip:bob%40biloxi.com",
            "sip:user;par=u%40example.net@example.com",
            "sips:alice@[2001:db8::1]:5061;maddr=239.255.255.1;ttl=15",
            "nobodyKnowsThisScheme:totally-bogus-stuff",
        ] {
            assert_eq!(
                uri(input).to_bytes(),
                Bytes::from(input.to_owned()),
                "{input} must survive a round trip unchanged"
            );
        }
    }

    #[test]
    fn a_constructed_uri_serializes_from_its_parts() {
        let mut u = Uri::sip(Host::Name(
            HostName::new(Bytes::from_static(b"example.com")).expect("a valid host"),
        ));
        u.push_param(Param::new(
            Bytes::from_static(b"transport"),
            Bytes::from_static(b"tcp"),
        ));
        assert_eq!(
            u.to_bytes(),
            Bytes::from_static(b"sip:example.com;transport=tcp")
        );
    }

    /// The removal half of the pair. §19.1.1 forbids a repeated `uri-parameter`, so a caller
    /// re-setting one of its own must remove it first — and a removal that missed would produce a
    /// URI the far end cannot parse at all rather than one that merely says the wrong thing.
    #[test]
    fn removing_a_uri_parameter_reports_whether_there_was_one() {
        let mut u = uri("sip:alice@example.com;transport=tcp;lr");
        assert!(
            u.remove_param("TRANSPORT"),
            "§19.1.4 compares names case-insensitively"
        );
        assert_eq!(
            u.to_bytes(),
            Bytes::from_static(b"sip:alice@example.com;lr")
        );
        assert!(!u.remove_param("transport"), "it was already gone");
        assert!(u.remove_param("lr"));
        assert_eq!(u.to_bytes(), Bytes::from_static(b"sip:alice@example.com"));
        // A scheme sipx does not model has no uri-parameter list, so there is nothing to remove
        // and nothing is claimed — the same no-op `push_param` is on one.
        let mut opaque = uri("tel:+15551234");
        assert!(!opaque.remove_param("transport"));
        assert_eq!(opaque.to_bytes(), Bytes::from_static(b"tel:+15551234"));
    }

    #[test]
    fn mutating_a_parsed_uri_drops_its_verbatim_form() {
        let mut u = uri("sip:alice@Example.COM");
        u.push_param(Param::flag(Bytes::from_static(b"lr")));
        // The host keeps its original spelling because the parts hold the original bytes;
        // what is lost is only the guarantee of byte-for-byte reproduction.
        assert_eq!(
            u.to_bytes(),
            Bytes::from_static(b"sip:alice@Example.COM;lr")
        );
    }

    /// RFC 3261 §19.1.1: "any given parameter-name MUST NOT appear more than once" among
    /// uri-parameters. Accepting a repeat also made equivalence irreflexive, because each
    /// occurrence was compared against the other URI's *first* one.
    #[test]
    fn duplicate_uri_parameter_names_are_rejected() {
        for input in [
            "sip:h;a=1;a=2",
            "sip:h;a=1;a=1",
            "sip:h;lr;lr",
            // Case and escape spellings of a name are still that name (§19.1.4).
            "sip:h;transport=udp;TRANSPORT=tcp",
            "sip:h;transport=udp;%74ransport=tcp",
        ] {
            assert!(
                Uri::parse(Bytes::from(input.to_owned())).is_err(),
                "{input} should be rejected"
            );
        }
        // URI *headers* may repeat; only uri-parameters are policed.
        assert!(Uri::parse(Bytes::from_static(b"sip:a?f=a&f=b")).is_ok());
    }

    /// `;;` is not a quirky spelling of `;`. The ABNF's pname is `1*paramchar`, and the
    /// header grammar takes the same line — RFC 4475 3.1.2.1 makes a message invalid on
    /// exactly this, in a `Via`.
    #[test]
    fn empty_parameter_segments_are_rejected() {
        for input in ["sip:host;;a=1", "sip:host;a=1;", "sip:host;", "sip:host;;"] {
            assert_eq!(
                Uri::parse(Bytes::from(input.to_owned())).err(),
                Some(UriError::EmptyParameterName),
                "{input} should be rejected"
            );
        }
        // A single well-formed parameter is of course still fine.
        assert_eq!(uri("sip:host;a=1").params().map(Params::len), Some(1));
    }
}
