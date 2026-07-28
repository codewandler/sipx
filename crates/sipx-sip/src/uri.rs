//! SIP, SIPS and other URIs (RFC 3261 §19.1).

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use bytes::Bytes;

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
    /// The exact bytes this URI was parsed from, so a forwarded URI is re-emitted unchanged.
    /// `None` for a URI this process constructed.
    raw: Option<Bytes>,
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

        let parts = if scheme.is_sip() {
            Parts::Sip(Box::new(parse_sip_parts(&rest)?))
        } else {
            Parts::Opaque(rest)
        };

        Ok(Self {
            scheme,
            parts,
            raw: Some(raw),
        })
    }

    /// Build a `sip:` or `sips:` URI.
    #[must_use]
    pub fn sip(host: Host) -> Self {
        Self {
            scheme: Scheme::Sip,
            parts: Parts::Sip(Box::new(SipParts {
                user: None,
                password: None,
                host,
                port: None,
                params: Params::new(),
                headers: Params::new(),
            })),
            raw: None,
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
        // Any mutation invalidates the verbatim form.
        self.raw = None;
        match &mut self.parts {
            Parts::Sip(p) => Some(p),
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

    /// The value of the `transport` parameter.
    #[must_use]
    pub fn transport(&self) -> Option<&[u8]> {
        self.params().and_then(|p| p.value("transport"))
    }

    /// Add a URI parameter.
    pub fn push_param(&mut self, param: Param) {
        if let Some(parts) = self.sip_parts_mut() {
            parts.params.push(param);
        }
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
    let (number_a, params_a) = split_tel_body(a);
    let (number_b, params_b) = split_tel_body(b);

    if !escape::eq_ignore_ascii_case(
        &strip_visual_separators(number_a),
        &strip_visual_separators(number_b),
    ) {
        return false;
    }

    tel_param_multiset(params_a) == tel_param_multiset(params_b)
}

/// Split a tel URI body into the telephone-subscriber part and the parameter tail.
#[must_use]
fn split_tel_body(body: &[u8]) -> (&[u8], &[u8]) {
    match body.iter().position(|&b| b == b';') {
        Some(semi) => (
            body.get(..semi).unwrap_or(&[]),
            body.get(semi + 1..).unwrap_or(&[]),
        ),
        None => (body, &[]),
    }
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

/// Hostname characters.
///
/// The ABNF permits only alphanumerics, `-` and `.`. sipx also accepts `_`, which the grammar
/// does not: it is common in deployed hostnames, and it is not a delimiter anywhere in the
/// URI grammar, so accepting it cannot make a URI ambiguous.
#[must_use]
fn is_host_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_')
}

fn parse_sip_parts(rest: &Bytes) -> Result<SipParts, UriError> {
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
        password,
        host,
        port,
        params,
        headers,
    })
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
        let ip: Ipv6Addr = text.parse().map_err(|_| UriError::Host)?;
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

    #[test]
    fn parses_ipv4_literal_as_an_address() {
        let u = uri("sip:bob@192.0.2.4");
        assert!(matches!(u.host(), Some(Host::Ip(IpAddr::V4(_)))));
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
