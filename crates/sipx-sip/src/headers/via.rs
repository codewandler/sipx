//! The `Via` header (RFC 3261 §20.42).
//!
//! ```abnf
//! via-parm      = sent-protocol LWS sent-by *( SEMI via-params )
//! sent-protocol = protocol-name SLASH protocol-version SLASH transport
//! sent-by       = host [ COLON port ]
//! SLASH         = SWS "/" SWS
//! ```
//!
//! `SLASH` and `LWS` mean whitespace is legal almost everywhere, including across a line
//! fold: RFC 4475 §3.1.1.1 sends `Via  : SIP  /   2.0` with the `/UDP` on the next line. The
//! top `Via` decides where a response goes and its `branch` identifies the transaction, so
//! this is the header the transaction layer leans on hardest.

use bytes::Bytes;
use std::fmt;

use crate::error::HeaderError;
use crate::headers::grammar::{self, HeaderParam, find_param_start, skip_ws, trim};
use crate::message::TypedHeader;
use crate::name::HeaderName;
use crate::uri::Host;

/// The magic cookie that marks a branch parameter as RFC 3261 rather than RFC 2543
/// (RFC 3261 §8.1.1.7). Its absence is what selects the legacy matching rules.
pub const BRANCH_MAGIC_COOKIE: &[u8] = b"z9hG4bK";

const LABEL: &str = "Via";
const OC_SEQUENCE_SCALE: u64 = 100_000;

/// The overload-control capability or value carried by `Via`'s `oc` parameter (RFC 7339 §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcParameter {
    /// A valueless parameter: the client supports overload control.
    Support,
    /// A server report. Its units are selected by [`OverloadAlgorithm`].
    Value(u64),
}

/// One algorithm token from `oc-algo` (RFC 7339 §4.2, RFC 7415 §3.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverloadAlgorithm {
    /// Percentage loss control.
    Loss,
    /// Requests-per-second rate control.
    Rate,
    /// An extension a peer advertised. Kept so negotiation can ignore rather than corrupt it.
    Other(Vec<u8>),
}

/// RFC 7339's decimal `oc-seq`, normalized to five fractional decimal places.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct OverloadSequence(u64);

impl OverloadSequence {
    /// Construct an integral sequence value for a locally generated report.
    #[must_use]
    pub fn from_integer(value: u64) -> Option<Self> {
        value.checked_mul(OC_SEQUENCE_SCALE).map(Self)
    }

    /// Parse `1*12DIGIT "." 1*5DIGIT` from RFC 7339 §13.1.
    pub fn parse(value: &[u8]) -> Result<Self, HeaderError> {
        let Some(dot) = value.iter().position(|byte| *byte == b'.') else {
            return Err(HeaderError::Syntax { header: LABEL });
        };
        let whole = value.get(..dot).unwrap_or(&[]);
        let fraction = value.get(dot.saturating_add(1)..).unwrap_or(&[]);
        if whole.is_empty()
            || whole.len() > 12
            || fraction.is_empty()
            || fraction.len() > 5
            || !whole.iter().all(u8::is_ascii_digit)
            || !fraction.iter().all(u8::is_ascii_digit)
        {
            return Err(HeaderError::Syntax { header: LABEL });
        }
        let whole = decimal_u64(whole)?;
        let fraction_value = decimal_u64(fraction)?;
        let missing = 5usize.saturating_sub(fraction.len());
        let scale = 10u64
            .checked_pow(u32::try_from(missing).unwrap_or(0))
            .ok_or(HeaderError::Syntax { header: LABEL })?;
        let scaled_fraction = fraction_value
            .checked_mul(scale)
            .ok_or(HeaderError::Syntax { header: LABEL })?;
        whole
            .checked_mul(OC_SEQUENCE_SCALE)
            .and_then(|base| base.checked_add(scaled_fraction))
            .map(Self)
            .ok_or(HeaderError::Syntax { header: LABEL })
    }
}

impl fmt::Display for OverloadSequence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0 / OC_SEQUENCE_SCALE;
        let fraction = self.0 % OC_SEQUENCE_SCALE;
        if fraction == 0 {
            return write!(formatter, "{whole}.0");
        }
        let mut digits = format!("{fraction:05}");
        while digits.ends_with('0') {
            digits.pop();
        }
        write!(formatter, "{whole}.{digits}")
    }
}

/// The four typed overload-control parameters on one `Via` hop.
#[derive(Debug, Clone, PartialEq)]
pub struct ViaOverload {
    /// `None` when `oc` is absent; otherwise capability or server value.
    pub oc: Option<OcParameter>,
    /// The offered list in a request or the single selected algorithm in a response.
    pub algorithms: Vec<OverloadAlgorithm>,
    /// Server-only validity. Absence is interpreted by the transport as 500 ms.
    pub validity: Option<std::time::Duration>,
    /// Server-only report sequence.
    pub sequence: Option<OverloadSequence>,
}

fn decimal_u64(value: &[u8]) -> Result<u64, HeaderError> {
    let text = std::str::from_utf8(value).map_err(|_| HeaderError::Syntax { header: LABEL })?;
    text.parse()
        .map_err(|_| HeaderError::Syntax { header: LABEL })
}

/// One `Via` value.
#[derive(Debug, Clone)]
pub struct Via {
    /// The protocol name, normally `SIP`.
    pub protocol: Vec<u8>,
    /// The protocol version, normally `2.0`.
    pub version: Vec<u8>,
    /// The transport: `UDP`, `TCP`, `TLS`, `SCTP`, `WS`, `WSS`, or anything else a peer sends
    /// — RFC 4475 §3.1.1.10 requires unknown transports to parse.
    pub transport: Vec<u8>,
    /// The host this hop wants responses sent to.
    pub host: Host,
    /// The port, if stated.
    pub port: Option<u16>,
    /// The via parameters.
    pub params: Vec<HeaderParam>,
}

impl Via {
    /// Decode the RFC 7339/RFC 7415 overload-control parameters on this hop.
    ///
    /// Presence and value of `oc` stay distinct because the client sends the former and only a
    /// server may send the latter. Invalid numbers are errors rather than zero: zero has protocol
    /// meaning for both loss/rate and validity.
    pub fn overload(&self) -> Result<ViaOverload, HeaderError> {
        let oc = match grammar::param(&self.params, "oc") {
            None => None,
            Some(parameter) => match parameter.value.as_deref() {
                None => Some(OcParameter::Support),
                Some(value) => Some(OcParameter::Value(decimal_u64(value)?)),
            },
        };
        let algorithms = match grammar::param(&self.params, "oc-algo") {
            None => Vec::new(),
            Some(parameter) => {
                let value = parameter
                    .value
                    .as_deref()
                    .ok_or(HeaderError::Syntax { header: LABEL })?;
                let algorithms: Vec<_> = value
                    .split(|byte| *byte == b',')
                    .map(|token| match grammar::trim(token) {
                        token if token.eq_ignore_ascii_case(b"loss") => OverloadAlgorithm::Loss,
                        token if token.eq_ignore_ascii_case(b"rate") => OverloadAlgorithm::Rate,
                        token => OverloadAlgorithm::Other(token.to_vec()),
                    })
                    .collect();
                if algorithms.iter().any(|algorithm| {
                    matches!(algorithm, OverloadAlgorithm::Other(token) if token.is_empty() || !token.iter().all(u8::is_ascii_alphanumeric))
                }) {
                    return Err(HeaderError::Syntax { header: LABEL });
                }
                algorithms
            }
        };
        let validity = grammar::param(&self.params, "oc-validity")
            .map(|parameter| {
                let value = parameter
                    .value
                    .as_deref()
                    .ok_or(HeaderError::Syntax { header: LABEL })?;
                decimal_u64(value).map(std::time::Duration::from_millis)
            })
            .transpose()?;
        let sequence = grammar::param(&self.params, "oc-seq")
            .map(|parameter| {
                parameter
                    .value
                    .as_deref()
                    .ok_or(HeaderError::Syntax { header: LABEL })
                    .and_then(OverloadSequence::parse)
            })
            .transpose()?;
        Ok(ViaOverload {
            oc,
            algorithms,
            validity,
            sequence,
        })
    }

    /// The `branch` parameter, which identifies the transaction.
    #[must_use]
    pub fn branch(&self) -> Option<&[u8]> {
        self.param("branch")
    }

    /// Whether the branch carries the RFC 3261 magic cookie.
    ///
    /// When it does not, the sender predates RFC 3261 and transaction matching must fall back
    /// to the rules in §17.2.3.
    #[must_use]
    pub fn has_rfc3261_branch(&self) -> bool {
        self.branch()
            .is_some_and(|b| b.starts_with(BRANCH_MAGIC_COOKIE))
    }

    /// The `received` parameter: the source address the previous hop was actually seen from
    /// (RFC 3261 §18.2.1).
    #[must_use]
    pub fn received(&self) -> Option<&[u8]> {
        self.param("received")
    }

    /// The `rport` parameter (RFC 3581). Present with no value in a request means "tell me
    /// what port you saw"; present with a value in a response is that port.
    #[must_use]
    pub fn rport(&self) -> Option<Option<&[u8]>> {
        grammar::param(&self.params, "rport").map(|p| p.value.as_deref())
    }

    /// The `maddr` parameter.
    #[must_use]
    pub fn maddr(&self) -> Option<&[u8]> {
        self.param("maddr")
    }

    /// The `ttl` parameter.
    #[must_use]
    pub fn ttl(&self) -> Option<&[u8]> {
        self.param("ttl")
    }

    /// Any via parameter, by name.
    #[must_use]
    pub fn param(&self, name: &str) -> Option<&[u8]> {
        grammar::param(&self.params, name).and_then(|p| p.value.as_deref())
    }

    /// Parse one `Via` value — a single hop, not a comma-separated list.
    pub fn parse_one(value: &[u8]) -> Result<Self, HeaderError> {
        let value = trim(value);
        if value.is_empty() {
            return Err(HeaderError::Syntax { header: LABEL });
        }

        let (before_params, params_tail) = match find_param_start(value) {
            Some(semi) => (
                value.get(..semi).unwrap_or(&[]),
                value.get(semi..).unwrap_or(&[]),
            ),
            None => (value, &[][..]),
        };

        // sent-protocol: three fields separated by slashes, with whitespace permitted around
        // each slash.
        let mut fields: Vec<&[u8]> = Vec::with_capacity(3);
        let mut start = 0usize;
        let mut split_count = 0usize;
        for (i, &b) in before_params.iter().enumerate() {
            if b == b'/' && split_count < 2 {
                fields.push(trim(before_params.get(start..i).unwrap_or(&[])));
                start = i + 1;
                split_count += 1;
            }
        }
        let tail = trim(before_params.get(start..).unwrap_or(&[]));
        if split_count != 2 {
            return Err(HeaderError::Syntax { header: LABEL });
        }

        // The third slash-separated field is `transport LWS sent-by`; the whitespace between
        // them is the only separator, and there may be a lot of it.
        let space = tail
            .iter()
            .position(|&b| matches!(b, b' ' | b'\t'))
            .ok_or(HeaderError::Syntax { header: LABEL })?;
        let transport = trim(tail.get(..space).unwrap_or(&[]));
        let sent_by = trim(tail.get(skip_ws(tail, space)..).unwrap_or(&[]));

        let protocol = fields.first().copied().unwrap_or(&[]);
        let version = fields.get(1).copied().unwrap_or(&[]);
        if protocol.is_empty() || version.is_empty() || transport.is_empty() || sent_by.is_empty() {
            return Err(HeaderError::Syntax { header: LABEL });
        }

        let (host, port) =
            Host::parse_hostport(&Bytes::copy_from_slice(sent_by)).map_err(|source| {
                HeaderError::Uri {
                    header: LABEL,
                    source,
                }
            })?;

        Ok(Self {
            protocol: protocol.to_vec(),
            version: version.to_vec(),
            transport: transport.to_vec(),
            host,
            port,
            params: grammar::parse_params(trim(params_tail), LABEL)?,
        })
    }

    /// Parse a header value that may carry several comma-separated hops.
    pub fn parse_list(value: &[u8]) -> Result<Vec<Self>, HeaderError> {
        grammar::split_list(value, LABEL)?
            .into_iter()
            .map(Self::parse_one)
            .collect()
    }
}

/// The index just past the first `via-parm` in a header value that may carry several
/// comma-separated hops.
///
/// A server has to add `received` and `rport` to the **topmost** hop and only that one, so it
/// needs to know where the first hop ends without reserializing the rest — the other hops
/// belong to other elements and must go back out exactly as they arrived.
///
/// Commas inside quoted parameter values are not separators, which is why this is not a call
/// to `position`.
#[must_use]
pub fn first_hop_end(value: &[u8]) -> usize {
    let mut i = 0usize;
    while i < value.len() {
        match value.get(i) {
            Some(b'"') => match grammar::quoted_string_end(value, i) {
                Some(end) => i = end,
                None => return value.len(),
            },
            Some(b',') => return i,
            Some(_) => i += 1,
            None => break,
        }
    }
    value.len()
}

impl TypedHeader for Via {
    const NAME: HeaderName = HeaderName::Via;

    /// Decodes the **first** hop in the value.
    ///
    /// A single `Via` header line may carry several comma-separated hops, so `typed::<Via>()`
    /// gives the topmost one — which is the one that matters for routing a response. Use
    /// [`Via::parse_list`] when every hop is needed.
    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        let parts = grammar::split_list(value, LABEL)?;
        let first = parts.first().copied().unwrap_or(&[]);
        Self::parse_one(first)
    }

    fn decode_list(value: &[u8]) -> Result<Vec<Self>, HeaderError> {
        Self::parse_list(value)
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

    fn via(value: &[u8]) -> Via {
        Via::parse_one(value).unwrap_or_else(|e| panic!("{value:?} should parse: {e}"))
    }

    #[test]
    fn parses_an_ordinary_via() {
        let v = via(b"SIP/2.0/UDP host5.example.com:5060;branch=z9hG4bKkdjuw");
        assert_eq!(v.protocol, b"SIP");
        assert_eq!(v.version, b"2.0");
        assert_eq!(v.transport, b"UDP");
        assert_eq!(v.port, Some(5060));
        assert_eq!(v.branch(), Some(&b"z9hG4bKkdjuw"[..]));
        assert!(v.has_rfc3261_branch());
    }

    /// RFC 4475 3.1.1.1 sends this, folded across three lines. Once unfolded the whitespace
    /// is still everywhere the grammar allows it.
    #[test]
    fn tolerates_whitespace_around_every_slash() {
        let v = via(b"SIP  /   2.0   /UDP     192.0.2.2;branch=390skdjuw");
        assert_eq!(v.protocol, b"SIP");
        assert_eq!(v.version, b"2.0");
        assert_eq!(v.transport, b"UDP");
        assert_eq!(v.branch(), Some(&b"390skdjuw"[..]));
        // No magic cookie: this sender predates RFC 3261 and matching must fall back.
        assert!(!v.has_rfc3261_branch());
    }

    /// RFC 4475 3.1.1.10: unknown transports are legal and must not be rejected.
    #[test]
    fn accepts_unknown_transports() {
        for transport in [&b"TLS"[..], b"SCTP", b"UNKNOWN", b"ws"] {
            let mut value = b"SIP/2.0/".to_vec();
            value.extend_from_slice(transport);
            value.extend_from_slice(b" host.example.com;branch=z9hG4bKx");
            assert_eq!(via(&value).transport, transport);
        }
    }

    /// RFC 4475 3.1.2.1: the stray separators make this invalid — but only because it is a
    /// `Via`. The same value under an unknown header name is legal.
    #[test]
    fn rejects_extraneous_separators() {
        assert!(Via::parse_list(b"SIP/2.0/UDP 192.0.2.15;;,;,,").is_err());
    }

    #[test]
    fn parses_several_hops_on_one_line() {
        let hops = Via::parse_list(
            b"SIP  / 2.0  / TCP     spindle.example.com   ;  branch  =   z9hG4bK9ikj8  , \
              SIP  /    2.0   / UDP  192.168.255.111   ; branch=z9hG4bK30239",
        )
        .expect("should parse");
        assert_eq!(hops.len(), 2);
        assert_eq!(hops[0].transport, b"TCP");
        assert_eq!(hops[1].branch(), Some(&b"z9hG4bK30239"[..]));
    }

    #[test]
    fn reports_rport_presence_separately_from_its_value() {
        // In a request rport is present and empty; in a response it carries the port.
        let asking = via(b"SIP/2.0/UDP h.example.com;rport;branch=z9hG4bKx");
        assert_eq!(asking.rport(), Some(None));

        let answered = via(b"SIP/2.0/UDP h.example.com;rport=1234;branch=z9hG4bKx");
        assert_eq!(answered.rport(), Some(Some(&b"1234"[..])));

        let absent = via(b"SIP/2.0/UDP h.example.com;branch=z9hG4bKx");
        assert_eq!(absent.rport(), None);
    }

    #[test]
    fn overload_parameters_are_typed_for_both_parties() {
        let offered = via(b"SIP/2.0/UDP client.example;branch=z9hG4bKx;oc;oc-algo=\"loss,rate\"")
            .overload()
            .expect("valid overload offer");
        assert_eq!(offered.oc, Some(OcParameter::Support));
        assert_eq!(
            offered.algorithms,
            vec![OverloadAlgorithm::Loss, OverloadAlgorithm::Rate]
        );
        assert_eq!(offered.validity, None);
        assert_eq!(offered.sequence, None);

        let report = via(
            b"SIP/2.0/UDP server.example;branch=z9hG4bKy;oc=37;oc-algo=rate;\
              oc-validity=750;oc-seq=42.125",
        )
        .overload()
        .expect("valid overload report");
        assert_eq!(report.oc, Some(OcParameter::Value(37)));
        assert_eq!(report.algorithms, vec![OverloadAlgorithm::Rate]);
        assert_eq!(report.validity, Some(std::time::Duration::from_millis(750)));
        assert_eq!(
            report.sequence,
            Some(OverloadSequence::parse(b"42.125").expect("sequence"))
        );
    }

    #[test]
    fn malformed_overload_numbers_are_not_zero() {
        for value in [
            b"SIP/2.0/UDP h;oc=not-a-number;oc-algo=loss".as_slice(),
            b"SIP/2.0/UDP h;oc=1.5;oc-algo=rate",
            b"SIP/2.0/UDP h;oc=10;oc-algo=loss;oc-validity=-1",
            b"SIP/2.0/UDP h;oc=10;oc-algo=loss;oc-seq=1",
        ] {
            assert!(via(value).overload().is_err(), "accepted {value:?}");
        }
    }

    #[test]
    fn rejects_malformed_sent_protocol() {
        for value in [
            &b"SIP/2.0 host.example.com"[..], // only one slash
            b"SIP/2.0/UDP",                   // no sent-by
            b"/2.0/UDP host",                 // empty protocol
            b"SIP//UDP host",                 // empty version
            b"SIP/2.0/ host",                 // empty transport
        ] {
            assert!(
                Via::parse_one(value).is_err(),
                "{value:?} should be rejected"
            );
        }
    }

    #[test]
    fn the_first_hop_ends_at_the_first_real_comma() {
        let single = b"SIP/2.0/UDP a;branch=x";
        assert_eq!(first_hop_end(single), single.len());
        let two = b"SIP/2.0/UDP a;branch=x, SIP/2.0/UDP b;branch=y";
        assert_eq!(&two[..first_hop_end(two)], b"SIP/2.0/UDP a;branch=x");
        // A comma inside a quoted parameter value is not a separator.
        let quoted = br#"SIP/2.0/UDP a;note="one, two";branch=x"#;
        assert_eq!(first_hop_end(quoted), quoted.len());
    }

    #[test]
    fn rejects_a_sent_by_with_a_bad_host() {
        assert!(Via::parse_one(b"SIP/2.0/UDP host:99999;branch=z9hG4bKx").is_err());
    }
}
