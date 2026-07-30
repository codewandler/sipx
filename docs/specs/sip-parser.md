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
- RFC 4291 §2.2 (IPv6 address text representation), which obsoletes the RFC 2373 grammar
  RFC 3261 §25.1 embedded — see §4.8.
- RFC 5118 (IPv6 acceptance corpus), §4.10 normatively requiring tolerance of the construct
  RFC 3261's inherited grammar derives.

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

**[RFC]** In a **request** line, exactly one SP separates the three elements. Multiple SPs
(§3.1.2.9, `lwsstart`) or a trailing SP (§3.1.2.10, `trws`) are malformed.
**[RFC]** In a **status** line the reason phrase may itself contain SP and HTAB, so the line is
split at the first two spaces only and everything after them is the reason. The request-line
strictness above must not be applied here.
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
`ParseError::HeaderSyntax`.

Note that stray separators *within* a header value are not a structural fault: `badinv01`
(§3.1.2.1) carries `Via: SIP/2.0/UDP 192.0.2.15;;,;,,`, which frames as an ordinary header
line and violates only the `Via` grammar. The identical value under an unknown header name is
legal — `wsinv`, a valid message, carries `UnknownHeaderWithUnusualValue: ;;,,;;,;`. The fault
is therefore unreachable without knowing which header it is, and belongs to the typed layer.

### 4.8 IPv6 references

Where a `host` may be an IPv6 reference — a Request-URI, a `Via` sent-by, any URI in an address
header — the text between `[` and `]` is an address and nothing else.

**[sipx] The `]` is what the parser keys on.** Everything inside the brackets is the address; a
port is read only *after* the `]`. So `sip:[2001:db8::10:5070]` names host `2001:db8::10:5070`
with **no** port, not host `2001:db8::10` on port 5070 — `5070` is the reference's last group
once `::` expands. RFC 5118 §4.3 states the sender will not get what it meant and that this is
nonetheless not a parse error: "From a parsing perspective, the request below is well-formed.
However, from a semantic point of view, it will not yield the desired result." §4.4 is the
contrast, `sip:[2001:db8::10]:5070`, where both halves are read. Any other reading collapses the
two sections into one.

**[RFC] The address grammar is RFC 4291 §2.2**, including embedded IPv4 (`::ffff:192.0.2.2`,
RFC 5118 §4.9). A reference that is not an RFC 4291 address is `UriError::Host`, with one
exception, below. In particular a reference with no `]` is `UriError::Ipv6Reference`, and an
undelimited IPv6 address in a Request-URI — `sip:2001:db8::10`, RFC 5118 §4.2 — is a start-line
fault, the only message in that corpus the RFC titles invalid.

**[RFC] `:::` is tolerated immediately before an embedded IPv4 address, and nowhere else.**
RFC 3261 §25.1 took its `IPv6address` production from the obsoleted RFC 2373:

```abnf
IPv6address = hexpart [ ":" IPv4address ]
hexpart     = hexseq / hexseq "::" [ hexseq ] / "::" [ hexseq ]
```

`hexpart` may end in `"::"` before the grammar appends `":" IPv4address`, so RFC 3261's own ABNF
derives `[2001:db8:::192.0.2.1]` — three colons. RFC 4291 corrected the grammar, but senders were
written against RFC 3261 and emit the third colon, and RFC 5118 §4.10 is normative that
"following the Robustness Principle [RFC1122], an implementation must tolerate both of the above
constructs."

sipx tolerates it as a **single documented carve-out**, not by relaxing the host rule:

| Reference | Result |
|---|---|
| `[2001:db8::192.0.2.1]` | `2001:db8::192.0.2.1` — RFC 4291, the correct construct |
| `[2001:db8:::192.0.2.1]` | `2001:db8::192.0.2.1` — the §4.10 carve-out |
| `[2001:db8:::10]` | `UriError::Host` — no embedded IPv4 address |
| `[2001:db8::::192.0.2.1]` | `UriError::Host` — four colons is not the derivation |
| `[2001:db8::1:::192.0.2.1]` | `UriError::Host` — the rewrite would need two `::` runs |
| `2001:db8:::192.0.2.1` (unbracketed) | `UriError::Host` — the carve-out is inside `[` `]` only |

**[sipx]** The mechanism is what keeps the table's bottom four rows honest: one `:::` is rewritten
to `::` and **retried through the same RFC 4291 parser**, which still has to accept the result.
There is no second address grammar, so the accepted language is exactly RFC 4291 plus that one
derivation. Rationale: reaching for a more permissive address parser would trade one unmet MUST
for an unmeasured surface on unauthenticated input, and sipx's posture is typed errors on network
input.

**[sipx] Tolerated is not normalised.** RFC 5118 §4.10 permits re-serializing the three-colon form
as two; sipx does not. A parsed URI keeps its verbatim bytes (`Uri::raw`), so a message forwarded
through sipx carries the reference its sender wrote. A parser that rewrote it unasked would have
altered a message it was only relaying.

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
| `ParseErr` | §3.1.2 structural | 8 | `parse_datagram` returns the specified `ParseError` |
| `HeaderErr` | §3.1.2 value-level | 7 | parses; the named header returns `HeaderError` |
| `ValidateErr` | §3.1.2 semantic | 4 | parses, headers parse; `validate_*` rejects |
| `ParseOk` | §3.2, §3.3, §3.4 | 14 | parses; behaviour belongs to later layers |
| `ParseErr` | §3.3.9 | 1 | `mcl01`, per the repeated-`Content-Length` rule in §4.4 |
| `ValidateErr` | §3.3.1, §3.3.8 | 2 | `insuf`, `multi01` |

Totals 49; the Appendix A archive holds a fiftieth file that no section references, carried
unclassified so the corpus stays a faithful copy of the archive.

One case is classified against the RFC's own description: `baddn` (§3.1.2.15) illustrates an
unquoted comma in a display name, but its archive file — alone among the fifty — has no
terminating blank line, so it fails while framing and never reaches the header layer. sipx
does not tolerate a missing terminator: on a datagram that is indistinguishable from a
truncated message, and on a stream it means "wait for more". The display-name fault is covered
by a hand-built message in the `From` header's tests instead.

The split between these classes is the point of the classification: `ncl` (negative
`Content-Length`) is a framing failure, `scalar02` (`CSeq` too large) is a message that frames
and forwards perfectly well and whose `CSeq` is bad, and `insuf` is a message where every
header is fine and the *set* of them is not. Conflating them produces a stack that either
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
