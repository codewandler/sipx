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

use crate::error::HeaderError;
use crate::headers::grammar::{self, HeaderParam, find_param_start, skip_ws, trim};
use crate::message::TypedHeader;
use crate::name::HeaderName;
use crate::uri::Host;

/// The magic cookie that marks a branch parameter as RFC 3261 rather than RFC 2543
/// (RFC 3261 §8.1.1.7). Its absence is what selects the legacy matching rules.
pub const BRANCH_MAGIC_COOKIE: &[u8] = b"z9hG4bK";

const LABEL: &str = "Via";

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
