//! Turning bytes into messages.
//!
//! One implementation serves both transports. [`parse_datagram`] frames a message from a
//! single packet; [`StreamParser`] frames messages out of a byte stream arriving in arbitrary
//! chunks. They share every rule, so a message parses identically however it arrived — a
//! property the tests assert directly by splitting each corpus message at every byte offset.
//!
//! See `docs/specs/sip-parser.md` for the normative rules and the reasoning behind the
//! choices the RFC leaves open.

use bytes::{Bytes, BytesMut};

use crate::error::{FramingError, HeaderSyntaxError, LimitKind, ParseError, StartLineError};
use crate::message::{Header, Headers, Message, Method, Request, Response, StatusCode, Version};
use crate::name::HeaderName;
use crate::uri::Uri;

/// Bounds on what the parser will accept.
///
/// Every limit is checked *before* the corresponding allocation. A declared `Content-Length`
/// above `max_body_bytes` is rejected without reserving that memory; otherwise a twelve-byte
/// header is a remote memory-exhaustion primitive.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Largest message accepted, headers and body together.
    pub max_message_bytes: usize,
    /// Largest body accepted.
    pub max_body_bytes: usize,
    /// Most header fields accepted.
    pub max_headers: usize,
    /// Largest single header field, folding included.
    pub max_header_bytes: usize,
    /// Most continuation lines in one header field.
    pub max_folding_lines: usize,
}

impl Limits {
    /// Defaults for datagram transports, where a message must fit one packet.
    #[must_use]
    pub fn datagram() -> Self {
        Self {
            max_message_bytes: 64 * 1024,
            max_body_bytes: 64 * 1024,
            max_headers: 256,
            max_header_bytes: 8 * 1024,
            max_folding_lines: 16,
        }
    }

    /// Defaults for stream transports, which may legitimately carry larger bodies.
    #[must_use]
    pub fn stream() -> Self {
        Self {
            max_message_bytes: 1024 * 1024,
            max_body_bytes: 1024 * 1024,
            ..Self::datagram()
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::datagram()
    }
}

/// Parse exactly one message from a datagram.
///
/// Octets after the body are ignored, not rejected: RFC 3261 §18.3 says a datagram carries at
/// most one message and the rest is noise, and RFC 4475 §3.1.1.8 makes a test of it. They are
/// not part of the message and are not forwarded.
// Takes the buffer by value on purpose: `Bytes` is a refcounted handle, and the parsed
// message keeps views into this exact allocation. Borrowing would suggest the caller still
// owns something it does not.
#[allow(clippy::needless_pass_by_value)]
pub fn parse_datagram(buf: Bytes, limits: &Limits) -> Result<Message, ParseError> {
    if buf.len() > limits.max_message_bytes {
        return Err(ParseError::Limit {
            limit: LimitKind::MessageBytes,
            value: buf.len(),
        });
    }

    let head_end = find_header_terminator(&buf, 0)
        .ok_or(ParseError::Framing(FramingError::NoHeaderTerminator))?;
    let head = buf.slice(..head_end);
    let rest = buf.slice(head_end + 4..);

    let (start, headers) = parse_head(head, limits)?;

    let body = if let Some(declared) = content_length(&headers)? {
        check_body_limit(declared, limits)?;
        let declared = usize::try_from(declared).unwrap_or(usize::MAX);
        if declared > rest.len() {
            return Err(ParseError::Framing(FramingError::BodyTruncated));
        }
        rest.slice(..declared)
    } else {
        // RFC 3261 §20.14: with no Content-Length on a datagram, the body runs to the end.
        check_body_limit(rest.len() as u64, limits)?;
        rest
    };

    Ok(assemble(start, headers, body))
}

/// Frames messages out of a byte stream.
///
/// Holds at most one partial message. Completed messages are split off the buffer with
/// [`BytesMut::split_to`], so each message owns a view of the same allocation rather than a
/// copy.
#[derive(Debug)]
pub struct StreamParser {
    buf: BytesMut,
    limits: Limits,
    state: State,
    /// How far the header-terminator search has already looked, so a stream arriving one byte
    /// at a time does not rescan the buffer each time.
    scanned: usize,
    failed: bool,
}

#[derive(Debug)]
enum State {
    Head,
    Body {
        pending: Box<Pending>,
        needed: usize,
    },
}

#[derive(Debug)]
struct Pending {
    start: StartLine,
    headers: Headers,
}

impl StreamParser {
    /// A parser with the given limits.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            buf: BytesMut::new(),
            limits,
            state: State::Head,
            scanned: 0,
            failed: false,
        }
    }

    /// Bytes buffered but not yet part of a completed message.
    ///
    /// Exposed so a transport can time out a peer that sends a header section and then stops
    /// — a slow-loris defence the parser cannot mount for itself.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.buf.len()
    }

    /// Append bytes, returning every message they completed, in order.
    ///
    /// An error is **fatal for the connection**: framing is lost and sipx does not attempt to
    /// resynchronize, because guessing where the next message starts is how a body becomes a
    /// request (RFC 4475 §3.1.2.3). Subsequent calls keep returning the same error.
    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Message>, ParseError> {
        if self.failed {
            return Err(ParseError::Framing(FramingError::NoHeaderTerminator));
        }
        self.buf.extend_from_slice(chunk);
        match self.drain() {
            Ok(messages) => Ok(messages),
            Err(e) => {
                self.failed = true;
                Err(e)
            }
        }
    }

    fn drain(&mut self) -> Result<Vec<Message>, ParseError> {
        let mut out = Vec::new();
        loop {
            match &self.state {
                State::Head => {
                    // RFC 3261 §7.5: CRLF before the start-line MUST be ignored on stream
                    // transports. RFC 5626 §4.4.1 makes CRLFCRLF the keepalive ping and a
                    // lone CRLF the pong, so peers send exactly this between messages.
                    // Dropped only between messages: within one, framing is untouched.
                    while self.buf.starts_with(b"\r\n") {
                        let _crlf = self.buf.split_to(2);
                        self.scanned = self.scanned.saturating_sub(2);
                    }
                    let Some(head_end) = find_header_terminator(&self.buf, self.scanned) else {
                        // Remember how far we looked. Back up three bytes so a terminator
                        // straddling the chunk boundary is still found.
                        self.scanned = self.buf.len().saturating_sub(3);
                        if self.buf.len() > self.limits.max_message_bytes {
                            return Err(ParseError::Limit {
                                limit: LimitKind::MessageBytes,
                                value: self.buf.len(),
                            });
                        }
                        return Ok(out);
                    };

                    let head = self.buf.split_to(head_end).freeze();
                    let _crlfcrlf = self.buf.split_to(4);
                    self.scanned = 0;

                    let (start, headers) = parse_head(head, &self.limits)?;
                    // On a stream the length is not optional: without it there is no way to
                    // know where this message ends and the next begins.
                    let declared = content_length(&headers)?
                        .ok_or(ParseError::Framing(FramingError::ContentLengthRequired))?;
                    check_body_limit(declared, &self.limits)?;
                    let needed = usize::try_from(declared).unwrap_or(usize::MAX);
                    self.state = State::Body {
                        pending: Box::new(Pending { start, headers }),
                        needed,
                    };
                }
                State::Body { needed, .. } => {
                    let needed = *needed;
                    if self.buf.len() < needed {
                        return Ok(out);
                    }
                    let body = self.buf.split_to(needed).freeze();
                    let State::Body { pending, .. } =
                        std::mem::replace(&mut self.state, State::Head)
                    else {
                        unreachable!("state was just observed to be Body")
                    };
                    let Pending { start, headers } = *pending;
                    out.push(assemble(start, headers, body));
                }
            }
        }
    }
}

/// A parsed start line.
#[derive(Debug)]
enum StartLine {
    Request {
        method: Method,
        uri: Box<Uri>,
        version: Version,
        raw: Bytes,
    },
    Response {
        version: Version,
        status: StatusCode,
        reason: Bytes,
        raw: Bytes,
    },
}

fn assemble(start: StartLine, headers: Headers, body: Bytes) -> Message {
    match start {
        StartLine::Request {
            method,
            uri,
            version,
            raw,
        } => Message::Request(Request::from_wire(
            method, *uri, version, raw, headers, body,
        )),
        StartLine::Response {
            version,
            status,
            reason,
            raw,
        } => Message::Response(Response::from_wire(
            version, status, reason, raw, headers, body,
        )),
    }
}

fn check_body_limit(declared: u64, limits: &Limits) -> Result<(), ParseError> {
    if declared > limits.max_body_bytes as u64 {
        return Err(ParseError::Limit {
            limit: LimitKind::BodyBytes,
            value: usize::try_from(declared).unwrap_or(usize::MAX),
        });
    }
    Ok(())
}

/// Index of the CRLFCRLF that ends the header section.
fn find_header_terminator(buf: &[u8], from: usize) -> Option<usize> {
    buf.get(from..)
        .and_then(|tail| tail.windows(4).position(|w| w == b"\r\n\r\n"))
        .map(|i| i + from)
}

/// Parse the start line and header fields.
///
/// `head` is everything before the terminating CRLFCRLF, so the start line and header fields
/// are separated by CRLF and there is no trailing CRLF.
#[allow(clippy::needless_pass_by_value)] // same reasoning as parse_datagram
fn parse_head(head: Bytes, limits: &Limits) -> Result<(StartLine, Headers), ParseError> {
    validate_line_endings(&head)?;

    let lines = split_folded_lines(&head, limits)?;
    let mut lines = lines.into_iter();
    let (start_from, start_to) = lines.next().ok_or(StartLineError::Empty)?;
    let start = parse_start_line(head.slice(start_from..start_to))?;

    let mut headers = Headers::new();
    for (index, (from, to)) in lines.enumerate() {
        if headers.len() >= limits.max_headers {
            return Err(ParseError::Limit {
                limit: LimitKind::Headers,
                value: headers.len() + 1,
            });
        }
        let line = head.slice(from..to);
        if line.len() > limits.max_header_bytes {
            return Err(ParseError::Limit {
                limit: LimitKind::HeaderBytes,
                value: line.len(),
            });
        }
        headers.push(parse_header_line(line, index + 2)?);
    }

    Ok((start, headers))
}

/// Reject any bare CR or bare LF.
///
/// SIP is a CRLF protocol. Accepting a bare LF as a terminator would let two elements
/// disagree about where a message ends, which is the classic request-smuggling shape.
fn validate_line_endings(head: &[u8]) -> Result<(), ParseError> {
    let mut i = 0;
    while let Some(&b) = head.get(i) {
        match b {
            b'\r' if head.get(i + 1) == Some(&b'\n') => i += 2,
            b'\r' | b'\n' => {
                return Err(ParseError::HeaderSyntax {
                    line: 1 + head.get(..i).map_or(0, count_lines),
                    kind: HeaderSyntaxError::BareNewline,
                });
            }
            _ => i += 1,
        }
    }
    Ok(())
}

fn count_lines(prefix: &[u8]) -> usize {
    prefix.windows(2).filter(|w| *w == b"\r\n").count()
}

/// Split into logical lines, joining continuations.
///
/// A line that begins with SP or HTAB continues the one before it (RFC 3261 §7.3.1), so its
/// bytes stay part of the previous span — folding and all, because the span is what gets
/// written back on the wire.
fn split_folded_lines(head: &[u8], limits: &Limits) -> Result<Vec<(usize, usize)>, ParseError> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut folds_in_line = 0usize;

    if matches!(head.first(), Some(b' ' | b'\t')) {
        return Err(ParseError::HeaderSyntax {
            line: 1,
            kind: HeaderSyntaxError::LeadingFold,
        });
    }

    while i < head.len() {
        if head.get(i) == Some(&b'\r') && head.get(i + 1) == Some(&b'\n') {
            if matches!(head.get(i + 2), Some(b' ' | b'\t')) {
                folds_in_line += 1;
                if folds_in_line > limits.max_folding_lines {
                    return Err(ParseError::Limit {
                        limit: LimitKind::FoldingLines,
                        value: folds_in_line,
                    });
                }
                i += 2;
                continue;
            }
            lines.push((start, i));
            i += 2;
            start = i;
            folds_in_line = 0;
        } else {
            i += 1;
        }
    }
    if start < head.len() {
        lines.push((start, head.len()));
    }
    Ok(lines)
}

fn parse_start_line(line: Bytes) -> Result<StartLine, ParseError> {
    if line.is_empty() {
        return Err(StartLineError::Empty.into());
    }

    // A line opening with the version is a status line. The match is case-insensitive
    // because the SIP-Version string is (RFC 3261 §7.1), and it cannot swallow a request:
    // no method token may contain the `/`.
    if line
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"SIP/"))
    {
        return parse_status_line(line);
    }

    // A request line is exactly three elements separated by one SP each. Splitting on SP and
    // demanding three non-empty parts rejects both multiple separators (RFC 4475 §3.1.2.9)
    // and a trailing space (§3.1.2.10) without a special case for either.
    let mut bounds = Vec::new();
    let mut start = 0usize;
    for (i, &b) in line.iter().enumerate() {
        if b == b' ' {
            bounds.push((start, i));
            start = i + 1;
        }
    }
    bounds.push((start, line.len()));

    if bounds.len() != 3 || bounds.iter().any(|(a, b)| a == b) {
        return Err(StartLineError::RequestLineShape.into());
    }
    let Some(&(m0, m1)) = bounds.first() else {
        return Err(StartLineError::RequestLineShape.into());
    };
    let Some(&(u0, u1)) = bounds.get(1) else {
        return Err(StartLineError::RequestLineShape.into());
    };
    let Some(&(v0, v1)) = bounds.get(2) else {
        return Err(StartLineError::RequestLineShape.into());
    };

    let method_raw = line.slice(m0..m1);
    if !method_raw.iter().all(|&b| is_token_char(b)) {
        return Err(StartLineError::Method.into());
    }
    let uri = Uri::parse(line.slice(u0..u1)).map_err(StartLineError::Uri)?;

    Ok(StartLine::Request {
        method: Method::parse(&method_raw),
        uri: Box::new(uri),
        version: Version::parse(&line.slice(v0..v1)),
        raw: line,
    })
}

fn parse_status_line(line: Bytes) -> Result<StartLine, ParseError> {
    // The reason phrase may itself contain spaces and tabs, so the line is cut at the first
    // two spaces only. The request-line strictness above must not be applied here.
    let first = line
        .iter()
        .position(|&b| b == b' ')
        .ok_or(StartLineError::MissingStatusCode)?;
    let version = Version::parse(&line.slice(..first));

    let after = line.slice(first + 1..);
    let (code_raw, reason) = match after.iter().position(|&b| b == b' ') {
        Some(second) => (after.slice(..second), after.slice(second + 1..)),
        // No second space: the reason phrase is absent rather than empty. The RFC's grammar
        // wants the space, but a missing empty reason is unambiguous and costs nothing to
        // accept, and the line is written back verbatim regardless.
        None => (after.clone(), Bytes::new()),
    };

    if code_raw.len() != 3 || !code_raw.iter().all(u8::is_ascii_digit) {
        return Err(StartLineError::StatusCode.into());
    }
    let value = code_raw
        .iter()
        .fold(0u16, |acc, &b| acc * 10 + u16::from(b - b'0'));
    let status = StatusCode::new(value).ok_or(StartLineError::StatusCode)?;

    Ok(StartLine::Response {
        version,
        status,
        reason,
        raw: line,
    })
}

fn parse_header_line(line: Bytes, line_number: usize) -> Result<Header, ParseError> {
    let colon = line
        .iter()
        .position(|&b| b == b':')
        .ok_or(ParseError::HeaderSyntax {
            line: line_number,
            kind: HeaderSyntaxError::MissingColon,
        })?;

    // HCOLON permits whitespace before the colon: `Content-Length   : 150` is legal.
    let mut name_end = colon;
    while name_end > 0 && matches!(line.get(name_end - 1), Some(b' ' | b'\t')) {
        name_end -= 1;
    }
    let name_raw = line.slice(..name_end);

    if name_raw.is_empty() {
        return Err(ParseError::HeaderSyntax {
            line: line_number,
            kind: HeaderSyntaxError::EmptyName,
        });
    }
    if !name_raw.iter().all(|&b| is_token_char(b)) {
        return Err(ParseError::HeaderSyntax {
            line: line_number,
            kind: HeaderSyntaxError::NameNotToken,
        });
    }

    // SWS after the colon: whitespace, possibly including a fold.
    let mut value_offset = colon + 1;
    loop {
        match line.get(value_offset) {
            Some(b' ' | b'\t') => value_offset += 1,
            Some(b'\r')
                if line.get(value_offset + 1) == Some(&b'\n')
                    && matches!(line.get(value_offset + 2), Some(b' ' | b'\t')) =>
            {
                value_offset += 2;
            }
            _ => break,
        }
    }

    Ok(Header::from_wire(
        HeaderName::parse(&name_raw),
        line,
        value_offset,
    ))
}

/// The declared body length, if the message states one.
fn content_length(headers: &Headers) -> Result<Option<u64>, ParseError> {
    let mut found: Option<u64> = None;
    for header in headers.get_all(&HeaderName::ContentLength) {
        if found.is_some() {
            return Err(ParseError::Framing(FramingError::ContentLengthRepeated));
        }
        let value = header.value();
        // Explicitly digits-only. A sign character is rejected here rather than by a signed
        // conversion, so there is no path on which a negative number becomes a length
        // (RFC 4475 §3.1.2.3 calls this out by name).
        if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
            return Err(ParseError::Framing(FramingError::ContentLengthMalformed));
        }
        let mut n: u64 = 0;
        for &b in value.iter() {
            n = n
                .checked_mul(10)
                .and_then(|n| n.checked_add(u64::from(b - b'0')))
                .ok_or(ParseError::Framing(FramingError::ContentLengthMalformed))?;
        }
        found = Some(n);
    }
    Ok(found)
}

/// RFC 3261 §25.1 `token`.
#[must_use]
fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'-' | b'.' | b'!' | b'%' | b'*' | b'_' | b'+' | b'`' | b'\'' | b'~'
        )
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
    use crate::error::LimitKind;

    fn parse(text: &str) -> Result<Message, ParseError> {
        parse_datagram(Bytes::from(text.to_owned()), &Limits::datagram())
    }

    const MINIMAL: &str = "OPTIONS sip:a@b.com SIP/2.0\r\n\
         Via: SIP/2.0/UDP h.example.com;branch=z9hG4bKx\r\n\
         To: <sip:a@b.com>\r\n\
         From: <sip:c@d.net>;tag=1\r\n\
         Call-ID: x@y\r\n\
         CSeq: 1 OPTIONS\r\n\
         Content-Length: 0\r\n\r\n";

    #[test]
    fn parses_a_minimal_request() {
        let msg = parse(MINIMAL).expect("should parse");
        let req = msg.as_request().expect("a request");
        assert_eq!(req.method, Method::Options);
        assert_eq!(req.version, Version::Sip20);
        assert_eq!(req.headers.len(), 6);
        assert!(msg.body().is_empty());
        assert_eq!(msg.to_bytes(), Bytes::from(MINIMAL));
    }

    #[test]
    fn parses_a_response_with_a_reason_containing_spaces() {
        let text = "SIP/2.0 486 Busy Here Right Now\r\nContent-Length: 0\r\n\r\n";
        let msg = parse(text).expect("should parse");
        let res = msg.as_response().expect("a response");
        assert_eq!(res.status.code(), 486);
        assert_eq!(res.reason, Bytes::from_static(b"Busy Here Right Now"));
        assert_eq!(msg.to_bytes(), Bytes::from(text));
    }

    #[test]
    fn accepts_an_empty_reason_phrase() {
        // RFC 4475 3.1.1.13: the reason may be empty, and the separating space stays.
        let text = "SIP/2.0 200 \r\nContent-Length: 0\r\n\r\n";
        let msg = parse(text).expect("should parse");
        assert!(msg.as_response().expect("a response").reason.is_empty());
        assert_eq!(msg.to_bytes(), Bytes::from(text));
    }

    #[test]
    fn rejects_bare_line_feeds() {
        let text = "OPTIONS sip:a@b.com SIP/2.0\nContent-Length: 0\r\n\r\n";
        assert!(matches!(
            parse(text),
            Err(ParseError::HeaderSyntax {
                kind: HeaderSyntaxError::BareNewline,
                ..
            })
        ));
    }

    #[test]
    fn rejects_a_header_section_starting_with_a_fold() {
        let text = " OPTIONS sip:a@b.com SIP/2.0\r\nContent-Length: 0\r\n\r\n";
        assert!(matches!(
            parse(text),
            Err(ParseError::HeaderSyntax {
                kind: HeaderSyntaxError::LeadingFold,
                ..
            })
        ));
    }

    #[test]
    fn content_length_faults_are_named_precisely() {
        let with = |cl: &str, body: &str| {
            format!("OPTIONS sip:a@b.com SIP/2.0\r\nContent-Length: {cl}\r\n\r\n{body}")
        };
        assert!(matches!(
            parse(&with("-999", "")),
            Err(ParseError::Framing(FramingError::ContentLengthMalformed))
        ));
        assert!(matches!(
            parse(&with("five", "")),
            Err(ParseError::Framing(FramingError::ContentLengthMalformed))
        ));
        assert!(matches!(
            parse(&with("", "")),
            Err(ParseError::Framing(FramingError::ContentLengthMalformed))
        ));
        assert!(matches!(
            parse(&with("9999", "short")),
            Err(ParseError::Framing(FramingError::BodyTruncated))
        ));
        // 2^64 overflows rather than wrapping to something plausible.
        assert!(matches!(
            parse(&with("18446744073709551616", "")),
            Err(ParseError::Framing(FramingError::ContentLengthMalformed))
        ));
    }

    #[test]
    fn repeated_content_length_is_rejected_even_when_the_values_agree() {
        let text = "OPTIONS sip:a@b.com SIP/2.0\r\n\
             Content-Length: 0\r\nContent-Length: 0\r\n\r\n";
        assert!(matches!(
            parse(text),
            Err(ParseError::Framing(FramingError::ContentLengthRepeated))
        ));
    }

    #[test]
    fn a_datagram_without_content_length_takes_the_rest_as_body() {
        // RFC 3261 §20.14.
        let text = "OPTIONS sip:a@b.com SIP/2.0\r\nTo: <sip:a@b.com>\r\n\r\nhello";
        let msg = parse(text).expect("should parse");
        assert_eq!(msg.body(), &Bytes::from_static(b"hello"));
        assert_eq!(msg.to_bytes(), Bytes::from(text));
    }

    #[test]
    fn trailing_octets_after_the_body_are_ignored() {
        // RFC 4475 3.1.1.8: a datagram carries one message; the rest is noise and must not be
        // forwarded.
        let text = "OPTIONS sip:a@b.com SIP/2.0\r\nContent-Length: 0\r\n\r\nINVITE sip:x@y SIP/2.0\r\n\r\n";
        let msg = parse(text).expect("should parse");
        assert!(msg.body().is_empty());
        let out = msg.to_bytes();
        assert!(text.as_bytes().starts_with(&out));
        assert!(out.len() < text.len(), "the noise must be dropped");
    }

    #[test]
    fn limits_are_enforced_before_allocation() {
        let limits = Limits {
            max_body_bytes: 10,
            ..Limits::datagram()
        };
        // A twelve-byte header claiming a gigabyte must not reserve a gigabyte.
        let text = "OPTIONS sip:a@b.com SIP/2.0\r\nContent-Length: 1073741824\r\n\r\n";
        assert!(matches!(
            parse_datagram(Bytes::from(text), &limits),
            Err(ParseError::Limit {
                limit: LimitKind::BodyBytes,
                ..
            })
        ));

        let limits = Limits {
            max_headers: 2,
            ..Limits::datagram()
        };
        let text = "OPTIONS sip:a@b.com SIP/2.0\r\nA: 1\r\nB: 2\r\nC: 3\r\n\r\n";
        assert!(matches!(
            parse_datagram(Bytes::from(text), &limits),
            Err(ParseError::Limit {
                limit: LimitKind::Headers,
                ..
            })
        ));
    }

    #[test]
    fn request_line_shape_is_strict() {
        for text in [
            "INVITE  sip:a@b.com SIP/2.0\r\n\r\n",  // two spaces
            "INVITE sip:a@b.com SIP/2.0 \r\n\r\n",  // trailing space
            "INVITE sip:a@b.com\r\n\r\n",           // missing version
            "INVITE <sip:a@b.com> SIP/2.0\r\n\r\n", // angle brackets
        ] {
            assert!(
                matches!(parse(text), Err(ParseError::StartLine(_))),
                "{text:?} should be rejected"
            );
        }
    }

    /// RFC 3261 §7.1: "The SIP-Version string is case-insensitive, but implementations MUST
    /// send upper-case." Receiving is the lenient half.
    #[test]
    fn sip_version_is_recognized_case_insensitively() {
        let text = "sip/2.0 200 OK\r\nContent-Length: 0\r\n\r\n";
        let msg = parse(text).expect("should parse");
        let res = msg
            .as_response()
            .expect("a lower-case version still marks a response");
        assert_eq!(res.status.code(), 200);
        assert!(res.version.is_supported());
        // The start line goes back out exactly as it arrived.
        assert_eq!(msg.to_bytes(), Bytes::from(text));

        let text = "OPTIONS sip:a@b.com sip/2.0\r\nContent-Length: 0\r\n\r\n";
        let msg = parse(text).expect("should parse");
        let req = msg.as_request().expect("a request");
        assert!(req.version.is_supported(), "sip/2.0 is SIP/2.0");
        assert_eq!(msg.to_bytes(), Bytes::from(text));
    }

    #[test]
    fn an_unknown_version_parses_so_the_caller_can_answer_505() {
        let text = "OPTIONS sip:a@b.com SIP/7.0\r\nContent-Length: 0\r\n\r\n";
        let msg = parse(text).expect("should parse");
        let req = msg.as_request().expect("a request");
        assert!(!req.version.is_supported());
        assert_eq!(req.version.as_bytes(), b"SIP/7.0");
    }

    #[test]
    fn stream_parser_requires_content_length() {
        let mut p = StreamParser::new(Limits::stream());
        let text = "OPTIONS sip:a@b.com SIP/2.0\r\nTo: <sip:a@b.com>\r\n\r\n";
        assert!(matches!(
            p.push(text.as_bytes()),
            Err(ParseError::Framing(FramingError::ContentLengthRequired))
        ));
    }

    #[test]
    fn stream_parser_returns_two_messages_from_one_chunk() {
        let mut p = StreamParser::new(Limits::stream());
        let both = format!("{MINIMAL}{MINIMAL}");
        let messages = p.push(both.as_bytes()).expect("should parse");
        assert_eq!(messages.len(), 2);
        assert_eq!(p.pending(), 0);
    }

    #[test]
    fn stream_parser_survives_split_at_every_offset() {
        let text = "INVITE sip:a@b.com SIP/2.0\r\n\
             Via: SIP/2.0/TCP h.example.com;branch=z9hG4bKx\r\n\
             Subject: folded\r\n  continuation\r\n\
             Content-Length: 5\r\n\r\nhello";
        let bytes = text.as_bytes();
        let whole = {
            let mut p = StreamParser::new(Limits::stream());
            let mut m = p.push(bytes).expect("should parse");
            assert_eq!(m.len(), 1);
            m.remove(0).to_bytes()
        };

        for split in 0..=bytes.len() {
            let mut p = StreamParser::new(Limits::stream());
            let (a, b) = bytes.split_at(split);
            let mut got = p.push(a).expect("first half");
            got.extend(p.push(b).expect("second half"));
            assert_eq!(
                got.len(),
                1,
                "split at {split} produced {} messages",
                got.len()
            );
            assert_eq!(
                got.first().map(Message::to_bytes),
                Some(whole.clone()),
                "split at {split} changed the message"
            );
        }
    }

    #[test]
    fn stream_parser_handles_one_byte_at_a_time() {
        let mut p = StreamParser::new(Limits::stream());
        let bytes = MINIMAL.as_bytes();
        let mut out = Vec::new();
        for i in 0..bytes.len() {
            let chunk = bytes.get(i..=i).expect("in range");
            out.extend(p.push(chunk).expect("should parse"));
        }
        assert_eq!(out.len(), 1);
        assert_eq!(
            out.first().map(Message::to_bytes),
            Some(Bytes::from(MINIMAL))
        );
    }

    /// RFC 3261 §7.5: CRLF before the start-line MUST be ignored on stream transports.
    /// RFC 5626 §4.4.1 makes CRLFCRLF the keepalive ping and a lone CRLF the pong, so these
    /// arrive routinely and must not poison the framing.
    #[test]
    fn stream_parser_ignores_crlf_before_the_start_line() {
        let mut p = StreamParser::new(Limits::stream());

        // A keepalive ping ahead of a message.
        let text = format!("\r\n\r\n{MINIMAL}");
        let messages = p
            .push(text.as_bytes())
            .expect("leading CRLFs are not an error");
        assert_eq!(messages.len(), 1);
        assert_eq!(p.pending(), 0);

        // A lone CRLF pong between messages, alone in its own chunk.
        assert!(p.push(b"\r\n").expect("a pong is not an error").is_empty());
        assert_eq!(p.pending(), 0);
        let messages = p.push(MINIMAL.as_bytes()).expect("framing must survive");
        assert_eq!(messages.len(), 1);
    }

    /// The same, however the bytes are chunked — the CRLFs and the terminator search must
    /// not disagree about offsets.
    #[test]
    fn leading_crlf_is_ignored_at_every_split_point() {
        let text = format!("\r\n\r\n\r\n{MINIMAL}");
        let bytes = text.as_bytes();
        for split in 0..=bytes.len() {
            let mut p = StreamParser::new(Limits::stream());
            let (a, b) = bytes.split_at(split);
            let mut got = p.push(a).expect("first half");
            got.extend(p.push(b).expect("second half"));
            assert_eq!(
                got.len(),
                1,
                "split at {split} produced {} messages",
                got.len()
            );
            assert_eq!(
                got.first().map(Message::to_bytes),
                Some(Bytes::from(MINIMAL)),
                "split at {split} changed the message"
            );
        }
    }

    #[test]
    fn a_body_containing_a_message_is_not_a_second_message() {
        let body = "INVITE sip:x@y SIP/2.0\r\n\r\n";
        let text = format!(
            "OPTIONS sip:a@b.com SIP/2.0\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut p = StreamParser::new(Limits::stream());
        let messages = p.push(text.as_bytes()).expect("should parse");
        assert_eq!(messages.len(), 1, "the body must not be reparsed");
        assert_eq!(
            messages.first().map(Message::body),
            Some(&Bytes::from(body))
        );
    }

    #[test]
    fn a_stream_framing_error_is_permanent() {
        let mut p = StreamParser::new(Limits::stream());
        let text = "OPTIONS sip:a@b.com SIP/2.0\r\nContent-Length: -1\r\n\r\n";
        assert!(p.push(text.as_bytes()).is_err());
        // Even a perfectly good message afterwards must not be accepted: the framing is lost
        // and guessing where the next message starts is how a body becomes a request.
        assert!(p.push(MINIMAL.as_bytes()).is_err());
    }
}
