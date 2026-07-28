//! Error types.
//!
//! Every rejection names what was wrong. A single opaque `Invalid` variant is not good
//! enough: the transaction layer chooses between 400, 413 and 505 based on which fault this
//! was, and an operator reading a log needs to know which byte offended.

use thiserror::Error;

/// A URI that could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UriError {
    /// No scheme, or a scheme that is not a token followed by `:`.
    #[error("missing or malformed URI scheme")]
    Scheme,
    /// A `sip:` or `sips:` URI with no host.
    #[error("URI has no host")]
    EmptyHost,
    /// The host contains a character no host may contain.
    #[error("invalid character in host")]
    Host,
    /// The port is not one to five digits, or exceeds 65535.
    #[error("invalid port")]
    Port,
    /// An IPv6 reference missing its closing bracket.
    #[error("unterminated IPv6 reference")]
    Ipv6Reference,
    /// A character illegal anywhere in a URI: whitespace, a control character, or one of
    /// `<`, `>`, `"`.
    #[error("illegal character in URI")]
    IllegalCharacter,
    /// A `%` not followed by two hex digits.
    #[error("malformed percent escape")]
    PercentEscape,
    /// A parameter or header with an empty name.
    #[error("empty parameter name")]
    EmptyParameterName,
}

/// A header whose value could not be parsed, or whose value is out of range.
///
/// Distinct from a parse error: the message framed correctly and this one header is bad. A
/// proxy may still forward such a message; only a party that needs to *read* the header has
/// a problem.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HeaderError {
    /// The value does not match the header's grammar.
    #[error("malformed {header} header")]
    Syntax {
        /// The header that failed to parse.
        header: &'static str,
    },
    /// The value parses but falls outside the range the RFC permits — a `CSeq` above
    /// 2^31-1, a `Max-Forwards` above 255, a status code outside 100..=699.
    #[error("{header} value out of range")]
    OutOfRange {
        /// The header whose value was out of range.
        header: &'static str,
    },
    /// A URI inside the header did not parse.
    #[error("invalid URI in {header} header: {source}")]
    Uri {
        /// The header carrying the URI.
        header: &'static str,
        /// Why the URI was rejected.
        #[source]
        source: UriError,
    },
    /// A quoted string with no closing quote.
    #[error("unterminated quoted string in {header} header")]
    UnterminatedQuotedString {
        /// The header carrying the unterminated string.
        header: &'static str,
    },
}
