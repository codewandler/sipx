# Spec: SIP message model

**Status:** normative · **Crate:** `sipx-sip` · **Story:** S-1 · **Design:**
[sip-core](../designs/sip-core.md)

Defines how sipx represents a SIP message in memory. The parser that produces these types is
specified in [sip-parser.md](sip-parser.md).

## 1. Normative references

- RFC 3261 §7 (SIP messages), §7.3 (header fields), §8.1.1 (required request headers),
  §19.1 (SIP and SIPS URIs), §20 (header field definitions), §25 (ABNF).
- RFC 3986 §2 (URI percent-encoding).
- RFC 3966 (`tel:` URIs).
- RFC 4475 (torture messages) — used as the acceptance corpus, not as a normative source.

**Out of scope:** SDP bodies (see `sdp.md`), header semantics belonging to the transaction
or dialog layers, and any header not listed in §5.

## 2. Design decisions

These are project decisions, not RFC requirements. Each is binding on the implementation.

**D1 — A message borrows the bytes it arrived in.**
A parsed message holds the original `Bytes` buffer plus an index of header spans. `Bytes`
slices are refcounted views, so a header value costs a pointer pair and no copy. Rationale:
a proxy forwards far more headers than it inspects; parsing every one into an owned `String`
allocates dozens of times per message for information nobody reads.

**D2 — Untouched headers are re-emitted byte for byte.**
Serialization of a parsed, unmodified message reproduces the input exactly, including header
order, original capitalization, compact forms, the whitespace around `:`, and line folding.
Rationale: forwarding must not rewrite what it does not understand; a stack that normalizes
whitespace breaks signature-bearing headers and complicates debugging. This property is
tested against every RFC 4475 message.

**D3 — Typed access is lazy and explicit.**
Parsing produces a structural message. Header *values* are parsed only when asked for. A
malformed `CSeq` therefore surfaces when `CSeq` is read, not when the message is parsed.
Rationale: a proxy must be able to forward a message containing a header it cannot parse, and
RFC 4475 §3.1.2.4 requires distinguishing "reject the message" from "ignore the field".

**D4 — Modification is per-header, and only modified headers lose byte-exactness.**
A header entry is either `Raw` (a span into the original buffer) or `Owned` (a value this
process constructed). Editing replaces one entry; every other entry keeps its span. Rationale:
adding a `Via` must not cost a full reserialize of the message.

**D5 — No panics, ever.**
Every fallible operation returns `Result`. Indexing is checked. `unsafe` is forbidden. This is
not negotiable: all input is hostile.

**D6 — Structural parsing does not validate application semantics.**
A message missing `To`, `From`, `Call-ID`, `CSeq` or `Via` parses successfully. Rejecting it
is `validate_request`'s job (§7). Rationale: RFC 4475 §3.3.1 places missing required headers
at the application layer, and the transaction layer must be able to build a 400 response —
which requires having parsed the message that is missing them.

## 3. Types

```rust
pub enum Message { Request(Request), Response(Response) }

pub struct Request {
    pub method: Method,
    pub uri: Uri,
    pub version: Version,
    pub headers: Headers,
    body: Bytes,
}

pub struct Response {
    pub version: Version,
    pub status: StatusCode,   // 100..=699
    pub reason: Bytes,        // MAY be empty (RFC 4475 §3.1.1.13)
    pub headers: Headers,
    body: Bytes,
}
```

`Method` is an enum of the RFC-defined methods plus `Other(Bytes)`. Method comparison is
**case-sensitive** (RFC 3261 §7.1): `Invite` and `INVITE` are different methods. Method tokens
may contain any `token` character — RFC 4475 §3.1.1.1 exercises
`!interesting-Method0123456789_*+`.%indeed'~`.

`Version` accepts only `SIP/2.0`. Anything else parses into `Version::Other` and is rejected
by `validate_*`; RFC 4475 §3.1.2.16 requires a 505, which needs the message parsed first (D6).

### 3.1 Headers

```rust
pub struct Headers { entries: Vec<HeaderEntry> }

struct HeaderEntry {
    name: HeaderName,        // resolved, canonical
    raw_name: Bytes,         // exactly as it appeared, for D2
    value: HeaderValue,
}

enum HeaderValue { Raw(Bytes), Owned(Vec<u8>) }
```

`Raw` holds the value span **as it appeared**, including any line folding. Unfolding happens
in a temporary buffer when a typed parse needs it (§4), so D2 survives.

Order is preserved absolutely, including the relative order of same-named headers. `Via` order
is load-bearing for routing; the implementation must never sort or deduplicate.

### 3.2 Header names

```rust
pub enum HeaderName { Via, From, To, CallId, CSeq, Contact, /* … */ Other(Bytes) }
```

- Comparison is ASCII case-insensitive (RFC 3261 §7.3.1).
- Compact forms (§20, §7.3.3) resolve to the same variant as their long form: `i`=Call-ID,
  `m`=Contact, `e`=Content-Encoding, `l`=Content-Length, `c`=Content-Type, `f`=From,
  `s`=Subject, `k`=Supported, `t`=To, `v`=Via, `r`=Refer-To, `b`=Referred-By,
  `o`=Event, `u`=Allow-Events, `j`=Reject-Contact, `d`=Request-Disposition,
  `x`=Session-Expires, `y`=Identity, `n`=Identity-Info, `a`=Accept-Contact.
- `Other` compares case-insensitively but preserves its bytes.
- Emitting a header sipx constructed uses the canonical long form. Emitting a parsed header
  uses `raw_name` (D2).

## 4. Typed access

```rust
impl Headers {
    pub fn get(&self, name: &HeaderName) -> Option<&HeaderValue>;   // first occurrence
    pub fn get_all(&self, name: &HeaderName) -> impl Iterator<Item = &HeaderValue>;
    pub fn typed<H: TypedHeader>(&self) -> Option<Result<H, HeaderError>>;
    pub fn typed_all<H: TypedHeader>(&self) -> impl Iterator<Item = Result<H, HeaderError>>;
}
```

`Option<Result<..>>` distinguishes *absent* from *present but malformed*; collapsing the two
is how implementations end up treating a corrupt `CSeq` as a missing one.

**Unfolding.** RFC 3261 §7.3.1 allows a header value to continue on following lines that begin
with whitespace. For typed parsing, each CRLF followed by whitespace is replaced by a single
SP. The raw span is unchanged.

**Comma-separated lists.** Headers whose grammar is `1#value` may appear either as repeated
header lines or as one line with comma-separated values, and the two forms are equivalent
(§7.3.1). `typed_all` flattens both. Splitting on commas must respect quoted strings, angle
brackets and comments — RFC 4475 §3.1.1.1 carries two `Via` values on one folded line, and
§3.1.2.6 an unterminated quoted string. Headers exempt from list splitting: `WWW-Authenticate`,
`Authorization`, `Proxy-Authenticate`, `Proxy-Authorization` (§7.3.1 explicitly), and any
header whose grammar is not a list.

## 5. Typed headers in scope

`Via`, `From`, `To`, `Call-ID`, `CSeq`, `Contact`, `Route`, `Record-Route`, `Max-Forwards`,
`Expires`, `Content-Type`, `Content-Length`, `Content-Encoding`, `Content-Disposition`,
`Allow`, `Supported`, `Require`, `Proxy-Require`, `Unsupported`, `Accept`, `Authorization`,
`WWW-Authenticate`, `Proxy-Authorization`, `Proxy-Authenticate`, `Refer-To`, `Referred-By`,
`Event`, `Subscription-State`, `Session-Expires`, `Min-SE`, `RSeq`, `RAck`, `Reason`, `Date`.

Everything else is `Other` and passes through untouched.

### 5.1 Scalar ranges

Out-of-range scalars are a `HeaderError`, never a wrap or a clamp (RFC 4475 §3.1.2.4/§3.1.2.5):

| Field | Range | On violation |
|---|---|---|
| `CSeq` sequence | `0..=2^31-1` (RFC 3261 §8.1.1.5) | error → caller sends 400 |
| `Max-Forwards` | `0..=255` | error; caller **may** proceed as if absent |
| `Expires`, `expires` param | `0..=2^32-1` | error; caller **may** use the default |
| status code | `100..=699` | parse error (§3.1.2.19) |

Leading zeros are legal: `0068` is 68, `0009` is 9 (§3.1.1.1). A value that is empty, signed,
or non-numeric is an error — never converted to a negative number and never used as a length
or index (§3.1.2.3).

### 5.2 Via

`Via` carries the fields the transaction and transport layers depend on: protocol name,
version, transport, sent-by host and port, and the parameters `branch`, `received`, `rport`,
`maddr`, `ttl`. Whitespace is permitted around every token — `SIP  /   2.0  /  UDP` split
across folded lines is one legal value (§3.1.1.1). Transport is an opaque token: `TLS`,
`SCTP` and unknown transports are all valid (§3.1.1.10).

## 6. Errors

```rust
pub enum ParseError {           // structural — see sip-parser.md
    StartLine(..), HeaderSyntax(..), Framing(..), Limit(..), Encoding(..),
}
pub enum HeaderError {          // one header's value
    Syntax { name: HeaderName }, OutOfRange { name: HeaderName }, Uri(UriError), …
}
```

Every rejection names *what* was wrong. A single opaque `Invalid` variant is not acceptable:
the transaction layer chooses between 400, 413 and 505 based on this.

## 7. Validation

`validate_request` and `validate_response` are separate from parsing (D6) and check what
RFC 3261 §8.1.1 requires: `To`, `From`, `CSeq`, `Call-ID`, `Max-Forwards` and `Via` present;
`CSeq` method matching the request line (§3.1.2.17, §3.1.2.18); `Via` present and parseable.
They return a list of findings so the caller can build one response naming the first fault.

## 8. Test vectors

Derived from the RFC 4475 corpus (see [sip-parser.md §7](sip-parser.md) for the full list).
The message model specifically must satisfy:

| # | Input | Requirement |
|---|---|---|
| M1 | `wsinv` | Round-trips byte-exactly; 2 `Via` values on one folded line; `MaX-fOrWaRdS` resolves to `Max-Forwards` and re-emits with original case |
| M2 | `intmeth` | Method `!interesting-Method0123456789_*+\`.%indeed'~` preserved exactly; UTF-8 in a display name and an extension header survives |
| M3 | `escnull` | `%00` in a URI user part is preserved as escaped; decoding never yields an interior NUL in a `&str` |
| M4 | `noreason` | Empty reason phrase parses; re-emits with the trailing space intact |
| M5 | `transports` | `TLS`, `SCTP`, `UNKNOWN` transports all parse |
| M6 | `scalar02` | `CSeq` > 2^31-1 is `OutOfRange`, and only when `CSeq` is read |
| M7 | `mcl01` | Two `Content-Length` headers are both retained by the model; rejection is the parser's job |
| M8 | `bext01` | Unknown `Require`/`Proxy-Require` values parse; acting on them is a later layer |
| M9 | any | `headers.get_all(Via)` yields values in wire order for repeated and comma-joined forms alike |
| M10 | any | Adding a `Via` leaves all other header spans untouched (assert by pointer identity) |
