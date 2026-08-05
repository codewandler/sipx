# Asserted and preferred identity header grammar

## 1. Scope and references

This specification defines the protocol-generic syntax boundary for asserted and preferred
identity fields. It deliberately does not decide whether an identity is trusted, which identity
should be asserted, whether privacy requires removal, or whether a preferred identity is
authorized. Those are application and trust-domain policy.

Normative sources:

- RFC 3325 §§9.1–9.2 define `P-Asserted-Identity` and `P-Preferred-Identity`, their shared
  `name-addr / addr-spec` grammar and the strict one-SIP-family-plus-one-TEL sending shape.
- RFC 5876 §4.5 updates receipt of the fields: unexpected schemes, duplicate schemes and a
  SIP/SIPS combination are ignored sequentially rather than making every valid identity unusable;
  a proxy MUST NOT forward an ignored value.
- RFC 8217 §§3–4 updates RFC 3325 and makes `name-addr` mandatory when a URI contains a comma,
  semicolon or question mark. This incorporates verified RFC 3325 errata 3744 and 3894.
- RFC 3261 §§7.3.1 and 25.1 define header-row combination, `name-addr`, `addr-spec`, SIP URI
  syntax, whitespace and quoted display names.
- RFC 3966 defines the `tel` URI syntax represented by the kernel URI type.

Considered for the application layer: no. Field-name recognition, address parsing, row combination,
strict construction and RFC 5876 receive filtering are protocol-generic, so they belong in
`sipx-sip`. Trust-domain behavior remains outside this specification.

## 2. Public types and the two contracts

`HeaderName` recognizes `P-Asserted-Identity` and `P-Preferred-Identity` and classifies both as
comma-separated list fields.

`PAssertedIdentity` and `PPreferredIdentity` each represent one **strictly constructible** RFC 3325
value. Their inner `Address` is private. `new(Address)` enforces a SIP, SIPS or TEL URI, no
header-parameter tail, a serializable display name and RFC 8217's bracket rule. Their deterministic
serializer always emits the unambiguous name-address form:

```text
[ quoted-display-name SP ] LAQUOT URI RAQUOT
```

The display name is omitted when absent. When present it is quoted; `\` and `"` are escaped. The
URI is emitted by `Uri::write_to`.

`PAssertedIdentityList::new` and `PPreferredIdentityList::new` are the complete strict construction
APIs. They accept only one value, or one SIP/SIPS value paired with one TEL value, and serialize the
usable values in order. The single-value types also implement `TypedHeader`; their
`Headers::typed_all` path is a **strict conformance diagnostic**, not the receive/forward contract.

`PAssertedIdentityList::from_headers` and `PPreferredIdentityList::from_headers` are the receive
APIs. They combine comma-joined and repeated rows in wire order and return:

- `Ok(None)` when the field is absent;
- usable typed values in receive order;
- every syntactically valid but ignored value as `IgnoredIdentity { index, address, reason }`, where
  `index` is the zero-based position in the combined field; and
- a typed `HeaderError` for an address, quote, URI or RFC 8217 syntax failure.

`requires_rewrite()` is true when forwarding the original field unchanged would violate RFC 5876
§4.5. The stable flattened indices are the seam consumed by parser-owned header surgery: a proxy
removes the ignored values in descending index order before forwarding rather than re-splitting the
field locally.
`to_bytes()` omits ignored values and returns `None` when none remain, making whole-field removal
explicit rather than offering an invalid empty header value.

## 3. Strict construction and diagnostic rules

| ID | Rule |
|---|---|
| C1 | A value is one `name-addr` or bare `addr-spec`, parsed by the common address and URI grammar. Percent escapes are well formed in every URI scheme, and a TEL telephone-subscriber satisfies RFC 3966 even though its parameter tail remains lossless and uninterpreted. |
| C2 | Its URI scheme is exactly `sip`, `sips` or `tel`. |
| C3 | A complete field contains one or two values. |
| C4 | Two values contain one SIP-family (`sip` or `sips`) value and one TEL value, in either order. |
| C5 | Value order is construction order; validation never sorts or deduplicates. |
| C6 | RFC 8217 applies independent of URI scheme: an `addr-spec` containing `,`, `;` or `?` is malformed unless enclosed in angle brackets. RFC 3325 defines no header-parameter tail. |
| C7 | A manually assembled `Address` cannot inject a field line through its display name: ASCII control bytes, malformed UTF-8 and a parameter tail are rejected before the private typed value is constructed. |
| C8 | `TypedHeader::decode_list` expands one row. Opt-in message-wide validation in `Headers::typed_all` makes the strict diagnostic apply across repeated rows too. Ordinary typed headers remain lazy and do not pay for this collection. Only SIP SP and HTAB are trimmed around values; other ASCII whitespace remains input and is rejected by the grammar. |

## 4. Receive filtering

All filtering is sequential over the combined field in wire order, as RFC 5876 §4.5 requires.
Ignored occurrences still count as earlier occurrences of their own scheme.

| ID | Received value | Result |
|---|---|---|
| R1 | First SIP or first SIPS URI, with no earlier URI from the other SIP family | usable |
| R2 | First TEL URI | usable |
| R3 | Scheme other than SIP, SIPS or TEL | ignored: `UnexpectedScheme` |
| R4 | Second or later SIP, SIPS or TEL URI | ignored: the matching `Duplicate*` reason |
| R5 | SIP after SIPS | ignored: `SipAfterSips` |
| R6 | SIPS after SIP | ignored: `SipsAfterSip` |
| R7 | Every ignored entry retains its parsed address, stable combined-field index and reason. Usable values retain their relative order. |
| R8 | A present field may yield zero usable values when every syntactically valid value is ignored; this remains distinct from absence. |
| R9 | Malformed address or URI syntax is not an “unexpected URI” and is returned as its precise `HeaderError`. No partial list is presented as complete. |
| R10 | A forwarding proxy MUST remove every ignored value. The receive result exposes `requires_rewrite` and indices so this obligation cannot be confused with successful parsing. |

## 5. State and I/O

There is no state machine, timer, clock, randomness or I/O. Decoding is a pure function of ordered
header bytes. Filtering is a pure left-to-right fold. Serialization is a pure function of checked
typed values.

## 6. Byte-level vectors

Unless stated otherwise the field is `P-Asserted-Identity`; the same rules apply to
`P-Preferred-Identity`.

| ID | Header rows or construction | Result |
|---|---|---|
| IH-1 | `<sip:alice@example.com>` | one strict SIP value; serializes `<sip:alice@example.com>` |
| IH-2 | `<sips:alice@example.com>` | one strict SIPS value |
| IH-3 | `tel:+12015550123` | one strict TEL value; serializes `<tel:+12015550123>` |
| IH-4 | `"Alice, A" <sip:alice@example.com>, <tel:+12015550123>` | two values; quoted comma is not a separator |
| IH-5 | one SIP row then one TEL row | the same strict pair in row order |
| IH-6 | TEL then SIPS on one row | valid strict pair in that order |
| IH-7 | the same pair in `P-Preferred-Identity` | identical strict contract |
| IH-8 | strict `<mailto:alice@example.com>` | `HeaderError::Syntax` |
| IH-9 | strict SIP plus SIPS | syntax error: two SIP-family values |
| IH-10 | strict two-TEL repeated rows | syntax error |
| IH-11 | strict SIP row plus TEL-and-SIP row | syntax error: three combined values |
| IH-12 | `<sip:a%GG@example.com>` | `HeaderError::Uri { source: UriError::PercentEscape, .. }` |
| IH-13 | unterminated quoted display name | `HeaderError::UnterminatedQuotedString` |
| IH-14 | bare `sip:alice@example.com;user=phone` | syntax error under RFC 8217; semicolon tail is not URI content |
| IH-15 | bare `sip:alice@example.com?subject=hello` | syntax error under RFC 8217 |
| IH-16 | bare `tel:+12015550123;ext=7` | syntax error under RFC 8217 |
| IH-17 | `<sip:alice@example.com;user=phone?subject=hello>` | accepted; URI parameter and header remain inside brackets; an outer `;tag=x` is rejected |
| IH-18 | receive MAILTO, SIP, TEL | SIP and TEL usable; MAILTO ignored at index 0 as `UnexpectedScheme`; rewrite required |
| IH-19 | receive SIP/SIPS then repeated SIP/TEL/TEL rows | SIP and first TEL usable; ignored indices and reasons are `(1, SipsAfterSip)`, `(2, DuplicateSip)`, `(4, DuplicateTel)` |
| IH-20 | receive SIPS then SIP in `P-Preferred-Identity` | SIPS usable; SIP ignored as `SipAfterSips` |
| IH-21 | absent field; all-unexpected field; malformed URI | respectively `None`; present with zero usable and one ignored; precise URI error |
| IH-22 | receive MAILTO/SIP then TEL/TEL across two rows; remove reported indices in descending order | only SIP and first TEL remain; a second receive pass requires no rewrite, proving the report drives the proxy MUST NOT forward surgery |
| IH-23 | complete construction from SIP plus SIPS, or three values | refused before serialization |
| IH-24 | complete construction from SIP plus TEL | serializes one deterministic comma-and-space-delimited row |
| IH-25 | construct one value from an address with CRLF or malformed UTF-8 in its display name | syntax error; no value constructed |
| IH-26 | a value surrounded by VT or FF, then receive and attempt indexed removal | both paths reject it as the same malformed address; non-SIP whitespace is never normalized away |
| IH-27 | bare TEL or MAILTO addr-spec containing `?`; bracketed MAILTO containing `?` | bare forms are RFC 8217 syntax errors independent of scheme; the bracketed valid unexpected scheme is reported as ignored |
| IH-28 | receive `<tel:>`, `<tel:+>`, `<mailto:%GG>` and `<mailto:alice@example.com>` | the first two are `UriError::TelephoneSubscriber`, the malformed escape is `UriError::PercentEscape`, and only the syntactically valid MAILTO is ignored as `UnexpectedScheme` |
