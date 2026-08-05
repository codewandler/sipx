//! Error types.
//!
//! Every rejection names what was wrong. A single opaque `Invalid` variant is not good
//! enough: the transaction layer chooses between 400, 413 and 505 based on which fault this
//! was, and an operator reading a log needs to know which byte offended.

use thiserror::Error;

/// A message that could not be built.
///
/// Every one of these means a caller tried to put something into a message that would have
/// changed its structure — the header-injection family. They are errors rather than silent
/// escaping because a caller that supplies a CRLF in a display name has a bug, and hiding it
/// helps nobody.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum BuildError {
    /// A response cannot be routed or correlated because the request omitted a header every
    /// response must copy (RFC 3261 §8.2.6.1 and §8.2.6.2).
    #[error("request is missing required response header {header}")]
    MissingRequiredResponseHeader {
        /// The missing header's canonical name.
        header: &'static str,
    },
    /// A character that would end a line or terminate a string, in a field that must not
    /// contain one.
    #[error("illegal byte {byte:#04x} at offset {offset} in {field}")]
    IllegalCharacter {
        /// Which field.
        field: &'static str,
        /// Where in the value.
        offset: usize,
        /// The offending byte.
        byte: u8,
    },
    /// A field that must be a single token is not one.
    #[error("{field} is not a token")]
    NotAToken {
        /// Which field.
        field: &'static str,
    },
}

/// A message that could not be framed or whose structure is malformed.
///
/// Structural only: a message whose *headers* are bad still parses (see [`HeaderError`]).
/// The transaction layer maps these onto a response status, which is why each variant says
/// what went wrong rather than merely that something did.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ParseError {
    /// The request or status line is malformed. Answer 400.
    #[error("malformed start line: {0}")]
    StartLine(#[from] StartLineError),
    /// A header field line is malformed. Answer 400.
    #[error("malformed header field on line {line}: {kind}")]
    HeaderSyntax {
        /// Which line of the header section, counting the start line as line 1.
        line: usize,
        /// What was wrong with it.
        kind: HeaderSyntaxError,
    },
    /// The message body cannot be delimited. Answer 400; on a stream transport the framing
    /// is unrecoverable and the connection must be closed.
    #[error("cannot frame message body: {0}")]
    Framing(#[from] FramingError),
    /// A configured limit was exceeded. Answer 413 for body limits.
    #[error("{limit} limit exceeded ({value})")]
    Limit {
        /// Which limit.
        limit: LimitKind,
        /// The value that exceeded it.
        value: usize,
    },
    /// Not enough bytes yet. Only ever returned internally by the stream parser; callers see
    /// it as "no message completed".
    #[error("incomplete message")]
    Incomplete,
}

/// What was wrong with a start line.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum StartLineError {
    /// The message is empty, or the start line is.
    #[error("empty")]
    Empty,
    /// A request line did not have exactly three space-separated elements. Covers multiple
    /// spaces between elements and a trailing space (RFC 4475 §3.1.2.9, §3.1.2.10).
    #[error("a request line must have exactly three space-separated elements")]
    RequestLineShape,
    /// The method is not a token.
    #[error("method is not a token")]
    Method,
    /// The Request-URI did not parse — including when it is wrapped in `<>`
    /// (RFC 4475 §3.1.2.7) or contains whitespace (§3.1.2.8).
    #[error("bad Request-URI: {0}")]
    Uri(#[from] UriError),
    /// A status line had no status code.
    #[error("status line has no status code")]
    MissingStatusCode,
    /// The status code is not exactly three digits in `100..=699` (RFC 4475 §3.1.2.19).
    #[error("status code is not three digits in 100..=699")]
    StatusCode,
}

/// What was wrong with a header field line.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum HeaderSyntaxError {
    /// No colon separating name from value.
    #[error("no colon")]
    MissingColon,
    /// The field name is empty.
    #[error("empty field name")]
    EmptyName,
    /// The field name contains a character outside the `token` set.
    #[error("field name is not a token")]
    NameNotToken,
    /// A bare CR or LF where a CRLF was required.
    ///
    /// sipx never accepts a bare LF as a line terminator: two elements disagreeing about
    /// where a message ends is how a body becomes a second request.
    #[error("bare CR or LF")]
    BareNewline,
    /// The first line of the header section begins with whitespace, so it continues a header
    /// that does not exist.
    #[error("header section begins with a continuation line")]
    LeadingFold,
}

/// Why a body could not be delimited.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum FramingError {
    /// No blank line terminating the header section.
    #[error("no blank line after headers")]
    NoHeaderTerminator,
    /// More than one `Content-Length`. Rejected even when the values agree: two elements
    /// having computed the same length is not worth a second code path (RFC 4475 §3.3.9).
    #[error("repeated Content-Length")]
    ContentLengthRepeated,
    /// `Content-Length` is empty, signed, or not a decimal number. Never converted to a
    /// number that could be negative, and never used as a length (RFC 4475 §3.1.2.3).
    #[error("Content-Length is not a decimal number")]
    ContentLengthMalformed,
    /// `Content-Length` is larger than the octets actually present (RFC 4475 §3.1.2.2).
    #[error("Content-Length exceeds the octets present")]
    BodyTruncated,
    /// A stream transport requires `Content-Length`; without it the stream cannot be cut into
    /// messages (RFC 3261 §20.14).
    #[error("Content-Length is required on stream transports")]
    ContentLengthRequired,
}

/// Which limit was exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitKind {
    /// Total message size.
    MessageBytes,
    /// Declared or actual body size.
    BodyBytes,
    /// Number of header fields.
    Headers,
    /// Size of a single header field.
    HeaderBytes,
    /// Number of continuation lines in one header field.
    FoldingLines,
}

impl std::fmt::Display for LimitKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::MessageBytes => "message size",
            Self::BodyBytes => "body size",
            Self::Headers => "header count",
            Self::HeaderBytes => "header size",
            Self::FoldingLines => "folding line count",
        };
        f.write_str(name)
    }
}

/// A URI that could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
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
    /// A SIP user part is empty or contains a byte outside RFC 3261's `user` production.
    #[error("invalid SIP URI user part")]
    User,
    /// A parsed or replacement `tel:` subscriber is empty or falls outside RFC 3966's global and
    /// local telephone-subscriber productions.
    #[error("invalid tel URI telephone-subscriber")]
    TelephoneSubscriber,
    /// A parsed message's retained URI span no longer points inside its retained wire bytes.
    #[error("retained URI span is inconsistent with its wire representation")]
    RetainedSpan,
    /// A parameter or header with an empty name.
    #[error("empty parameter name")]
    EmptyParameterName,
    /// A uri-parameter name that appears more than once. RFC 3261 §19.1.1: "any given
    /// parameter-name MUST NOT appear more than once." URI headers may repeat; only the
    /// `;` list is policed.
    #[error("repeated uri-parameter name")]
    DuplicateParameterName,
}

/// A header whose value could not be parsed, or whose value is out of range.
///
/// Distinct from a parse error: the message framed correctly and this one header is bad. A
/// proxy may still forward such a message; only a party that needs to *read* the header has
/// a problem.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
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

/// Why a parser-owned address value could not be edited losslessly.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum AddressEditError {
    /// The field does not use one of the address grammars exposed by this operation.
    #[error("header does not have a supported address grammar")]
    UnsupportedHeader,
    /// At least one row did not match the field's shared address grammar.
    #[error("malformed address field: {0}")]
    Malformed(#[source] HeaderError),
    /// The flattened, zero-based value index does not exist.
    #[error("address value index {index} is out of range")]
    IndexOutOfRange {
        /// The requested flattened value index.
        index: usize,
    },
    /// The replacement URI did not survive serialization as a valid URI.
    #[error("invalid replacement URI: {0}")]
    InvalidUri(#[source] UriError),
}
