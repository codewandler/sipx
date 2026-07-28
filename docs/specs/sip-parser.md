# Spec: SIP message parser

**Status:** normative · **Crate:** `sipx-sip` · **Story:** S-1, implemented by S-4 ·
**Design:** [sip-core](../designs/sip-core.md)

How bytes become the types in [sip-message.md](sip-message.md).

## 1. Normative references

- RFC 3261 §7.1–7.4 (message structure), §7.5 (framing), §18.3 (framing on datagram and
  stream transports), §20.14 (`Content-Length`), §25.1 (basic rules: `token`, `LWS`, `SWS`,
  quoted strings), §25 (ABNF).
- RFC 5234 (ABNF).
- RFC 4475 (acceptance corpus).

**Out of scope:** header *value* grammars (in `sip-message.md` §5 and implemented per header),
and transport behaviour on rejection (in `sip-transport.md`).

## 2. Interface

```rust
/// One message per datagram. Trailing octets are ignored (§4.4).
pub fn parse_datagram(buf: Bytes, limits: &Limits) -> Result<Message, ParseError>;

/// Incremental framing for stream transports.
pub struct StreamParser { /* … */ }
impl StreamParser {
    pub fn new(limits: Limits) -> Self;
    /// Append bytes. Returns messages completed by this call, in order.
    pub fn push(&mut self, chunk: Bytes) -> Result<Vec<Message>, ParseError>;
    /// Bytes buffered but not yet a complete message.
    pub fn pending(&self) -> usize;
}
```

A `ParseError` from `StreamParser::push` is **fatal for that connection**: framing is lost and
resynchronization is not attempted (RFC 4475 §3.1.2.3 — "the framing error is not recoverable,
and the connection should be closed"). A `ParseError` from `parse_datagram` affects only that
datagram.

## 3. Grammar

Only the structural layer is specified here; everything below is RFC 3261 §25 ABNF.

```abnf
SIP-message   =  Request / Response
Request       =  Request-Line   *( message-header CRLF ) CRLF [ message-body ]
Response      =  Status-Line    *( message-header CRLF ) CRLF [ message-body ]
Request-Line  =  Method SP Request-URI SP SIP-Version CRLF
Status-Line   =  SIP-Version SP Status-Code SP Reason-Phrase CRLF
message-header=  field-name HCOLON field-value
HCOLON        =  *( SP / HTAB ) ":" SWS
SWS           =  [ LWS ]
LWS           =  [ *WSP CRLF ] 1*WSP        ; line folding
```

A message is a Response if and only if the first bytes are `SIP/` followed by a version;
otherwise it is parsed as a Request.

## 4. Rules and decisions

Each decision is marked **[RFC]** (required) or **[sipx]** (our choice, with rationale).

### 4.1 Line endings

**[sipx] CRLF only.** A bare CR or bare LF is never a line terminator; encountering one where
a terminator is required is `ParseError::HeaderSyntax`.

Rationale: RFC 3261 §7 specifies CRLF, and tolerating bare LF is the classic request-smuggling
vector — two elements disagreeing about where a message ends is how a body becomes a second
request. Robustness here buys interoperability with broken senders at the cost of a security
property, and the vision's north star settles that trade.

### 4.2 Whitespace and folding

**[RFC]** `HCOLON` permits whitespace before and after the colon (`Content-Length   : 150`).
**[RFC]** A header value continues on any following line that starts with SP or HTAB; the
continuation is equivalent to a single SP. Folding may occur anywhere LWS is legal, including
mid-token in `SIP  /   2.0  /  UDP` (RFC 4475 §3.1.1.1).
**[sipx]** The raw span retains the folding; unfolding is done into a scratch buffer at typed
parse time, preserving byte-exact passthrough.
**[RFC]** A line whose first character is whitespace, before any header has been seen, is
malformed (§3.1.2.10, `lwsstart`).

### 4.3 Start line

**[RFC]** Exactly one SP separates the elements. Multiple SPs (§3.1.2.9, `multi01`) or a
trailing SP (§3.1.2.10, `trws`) are malformed.
**[RFC]** The Request-URI is not enclosed in `<>` (§3.1.2.7, `ltgtruri`) and contains no
whitespace (§3.1.2.8, `lwsruri`).
**[RFC]** The reason phrase MAY be empty, in which case the line ends `SIP/2.0 200 CRLF` with
the separating SP present (§3.1.1.13, `noreason`). It MAY also contain UTF-8 (§3.1.1.12).
**[RFC]** Status code is exactly three digits, `100..=699`; `bigcode` (§3.1.2.19) is rejected.
**[sipx]** An unknown SIP version parses (D6 in the message spec) and is rejected by
validation, so the caller can answer 505 rather than dropping the message.

### 4.4 Body framing

| Condition | Datagram | Stream |
|---|---|---|
| `Content-Length` present, ≤ remaining | body = that many octets; **trailing octets ignored** [RFC §18.3, §3.1.1.8] | body = that many octets; remainder stays buffered for the next message |
| `Content-Length` present, > available | `ParseError::Framing` [§3.1.2.2] | wait for more data; not an error |
| `Content-Length` absent | body = remainder of the datagram [RFC §20.14] | `ParseError::Framing` — length is mandatory on streams [RFC §20.14] |
| `Content-Length` negative, empty or non-numeric | `ParseError::Framing` [§3.1.2.3] | same, and fatal for the connection |
| `Content-Length` repeated | `ParseError::Framing`, **even if the values agree** [sipx] | same |

**[sipx]** on repeated `Content-Length`: RFC 4475 §3.3.9 permits a 400. Accepting agreeing
duplicates would be defensible, but "two elements computed the same length" is not a case
worth the divergence risk; rejecting uniformly is one rule instead of two.

**[sipx]** The value is parsed by an explicit ASCII-digit scan into `u64`, rejecting any sign
character before conversion, so no path exists on which a negative value becomes a length.
RFC 4475 §3.1.2.3 calls this out specifically.

### 4.5 Limits

```rust
pub struct Limits {
    pub max_message_bytes: usize,   // default 64 KiB (datagram), 1 MiB (stream)
    pub max_body_bytes: usize,      // default 1 MiB
    pub max_headers: usize,         // default 256
    pub max_header_bytes: usize,    // default 8 KiB per header, after unfolding
    pub max_folding_lines: usize,   // default 16 per header
}
```

**[sipx]** Every limit is checked *before* the corresponding allocation. A declared
`Content-Length` above `max_body_bytes` is rejected without reserving that memory — otherwise
a 12-byte header is a remote memory-exhaustion primitive. `longreq` (§3.1.1.7) is a legitimate
message with very long header values and must still parse under the defaults.

### 4.6 Character handling

**[RFC]** Header field values are octet strings. sipx does not require UTF-8 anywhere in a
message, and does not transcode. `intmeth` (§3.1.1.2) carries UTF-8 in a display name and an
extension header; `escnull` (§3.1.1.4) carries `%00` escapes.
**[sipx]** Percent-escapes are **not** decoded during parsing. Decoding is a URI-level
operation performed on request (`Uri::decoded_user()` and friends), and the decoded form is
returned as bytes, not `&str`. Rationale: `%00` must survive a round-trip unchanged, and
decoding into a Rust string type would either panic or lossily replace it.

### 4.7 Header names

**[RFC]** Case-insensitive; compact forms are equivalent to their long forms.
**[sipx]** A zero-length field name, or a name containing a character outside `token`, is
`ParseError::HeaderSyntax` (§3.1.2.1, `badinv01`).

## 5. Streaming

`StreamParser` must produce identical results regardless of how the byte stream is chunked.
The implementation buffers until it has a complete header section, parses it, then waits for
`Content-Length` octets.

**[sipx]** The parser holds at most one partial message. When a message completes, its buffer
is split with `Bytes::split_to`, so the completed message owns a view of the same allocation
and no copy occurs.

**[sipx]** `pending()` is exposed so the transport can enforce an idle timeout on a peer that
sends a header section and then stalls — a slow-loris defence the parser cannot mount itself.

## 6. Errors

```rust
pub enum ParseError {
    StartLine { kind: StartLineError },
    HeaderSyntax { line: usize, kind: HeaderSyntaxError },
    Framing(FramingError),
    Limit { limit: LimitKind, value: usize },
    Incomplete,                 // stream only; never returned to the caller
}
```

Each variant maps to a response status the transaction layer can send: `Framing` and
`HeaderSyntax` → 400, `Limit { max_body_bytes }` → 413, unknown version → 505.

## 7. Acceptance corpus

The RFC 4475 messages are decoded from the bit-exact archive in that RFC's Appendix A. Each
case carries an expected outcome:

| Class | RFC section | Count | Expectation |
|---|---|---|---|
| `ParseOk` | §3.1.1 | 13 | parses, and re-serializes byte-identically |
| `ParseErr` | §3.1.2 (structural) | 13 | `parse_datagram` returns the specified `ParseError` |
| `HeaderErr` | §3.1.2 (value-level) | 6 | parses; the named header returns `HeaderError` |
| `ParseOk` | §3.2, §3.3, §3.4 | 18 | parses; behaviour belongs to later layers |

The split between `ParseErr` and `HeaderErr` is the point of the classification: `ncl`
(negative `Content-Length`) is a framing failure, while `scalar02` (`CSeq` too large) is a
message that parses and whose `CSeq` is bad. Conflating them produces a stack that either
drops forwardable messages or accepts unframeable ones.

### 7.1 Structural test vectors

Beyond the corpus:

| # | Input | Expected |
|---|---|---|
| P1 | Any corpus message pushed to `StreamParser` one byte at a time | Identical result to a single push |
| P2 | Any corpus message split at every offset `0..len` | Identical result at every split |
| P3 | Two messages in one stream chunk | Both returned, in order |
| P4 | `INVITE sip:a@b SIP/2.0\nVia: …` (bare LF) | `HeaderSyntax` |
| P5 | Header section with no terminating CRLFCRLF | `Incomplete` (stream) / `Framing` (datagram) |
| P6 | `Content-Length: 18446744073709551616` | `Framing`, not a wrap |
| P7 | `Content-Length: 100000000` with 10 bytes of body, stream | `Incomplete` until the limit is hit, then `Limit` |
| P8 | 257 headers with `max_headers = 256` | `Limit { max_headers }` |
| P9 | Empty input | `Incomplete` (stream) / `StartLine` (datagram) |
| P10 | A message whose body contains `\r\n\r\nINVITE …` | Body preserved; no second message |
