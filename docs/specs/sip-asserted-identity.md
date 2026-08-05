# RFC 3325 asserted and preferred identity header grammar

## 1. Scope and references

This specification defines the syntax boundary for the RFC 3325 identity fields. It deliberately
does not decide whether an identity is trusted, which identity should be asserted, whether privacy
requires removal, or whether a preferred identity is authorized. Those are application and trust
domain policy.

Normative sources:

- RFC 3325 §§9.1–9.2 define `P-Asserted-Identity` and `P-Preferred-Identity`, their shared
  `name-addr / addr-spec` value grammar, their one-or-two cardinality and their scheme pairing.
- RFC 3261 §§7.3.1 and 25.1 define header-row combination, `name-addr`, `addr-spec`, SIP URI
  syntax, whitespace and quoted display names.
- RFC 3966 defines the `tel` URI syntax represented by the kernel URI type.

Considered for the application layer: no. Field-name recognition, address parsing, row combination
and the RFC 3325 scheme constraint are protocol-generic, so they belong in `sipx-sip`. Trust-domain
behavior remains outside this specification.

## 2. Public types

`HeaderName` recognizes `P-Asserted-Identity` and `P-Preferred-Identity` and classifies both as
comma-separated list fields.

`PAssertedIdentity` and `PPreferredIdentity` each represent one RFC 3325 value and expose its parsed
`Address` through an immutable accessor and `Deref`. Their inner address is private, and their
checked `new(Address)` constructors enforce the same scheme and parameter invariants as decoding.
Both implement `TypedHeader`; `decode_list` uses the common address-list grammar.
`Headers::typed_all::<H>()` combines every repeated row in wire order, expands comma-joined rows in
place, then asks `H::validate_list` to validate the complete message-wide list. The default validator
is a no-op for existing typed headers.

The identity types serialize one value deterministically as:

```text
[ quoted-display-name SP ] LAQUOT URI RAQUOT
```

The display name is omitted when absent. When present it is always quoted; `\` and `"` are escaped.
The URI is emitted by `Uri::write_to`. RFC 3325 adds no header parameters after the value, so such a
tail is rejected rather than silently retained. A bare SIP URI parameter remains part of the URI.

## 3. Validation rules

| ID | Rule |
|---|---|
| I1 | A value is one `name-addr` or bare `addr-spec`, parsed by the common address and URI grammar. |
| I2 | Its URI scheme is exactly `sip`, `sips` or `tel`, compared by the typed `Scheme` variant. |
| I3 | A present field has one or two values across all rows. Zero rows means the field is absent, not malformed. |
| I4 | One value may use any scheme from I2. |
| I5 | Two values contain one SIP-family (`sip` or `sips`) value and one `tel` value, in either order. Two values from one family are malformed. |
| I6 | Three or more values are malformed, including combinations split across repeated and comma-joined rows. |
| I7 | Value order is wire order and is also serialization order; validation never sorts or deduplicates. |
| I8 | A row-level address, quote or URI error remains that exact `HeaderError`. Message-wide I2–I6 failures are `HeaderError::Syntax` naming the field. No malformed field panics. |
| I9 | Construction is checked: an address using another scheme or carrying a header-parameter tail cannot become a typed identity value and therefore cannot serialize as if it were valid. |

## 4. State and I/O

There is no state machine, timer, clock, randomness or I/O. Decoding is a pure function of ordered
header bytes. Serialization is a pure function of a typed value.

## 5. Byte-level vectors

Unless stated otherwise the field is `P-Asserted-Identity`; the same rules apply to
`P-Preferred-Identity`.

| ID | Header rows | Result |
|---|---|---|
| IH-1 | `P-Asserted-Identity: <sip:alice@example.com>` | one SIP-family value; serializes `<sip:alice@example.com>` |
| IH-2 | `P-Asserted-Identity: <sips:alice@example.com>` | one SIP-family value |
| IH-3 | `P-Asserted-Identity: tel:+12015550123` | one tel value; serializes `<tel:+12015550123>` |
| IH-4 | `P-Asserted-Identity: "Alice, A" <sip:alice@example.com>, <tel:+12015550123>` | two values; the quoted comma is not a separator; serialization preserves SIP then tel order |
| IH-5 | `P-Asserted-Identity: <sip:alice@example.com>` then `P-Asserted-Identity: <tel:+12015550123>` | the same two typed values as IH-4, in row order |
| IH-6 | `P-Asserted-Identity: <tel:+12015550123>, <sips:alice@example.com>` | valid; tel-first order is preserved |
| IH-7 | `P-Preferred-Identity: <sip:alice@example.com>, <tel:+12015550123>` | the preferred type enforces the same list rules |
| IH-8 | `P-Asserted-Identity: <mailto:alice@example.com>` | `HeaderError::Syntax { header: "P-Asserted-Identity" }` |
| IH-9 | `P-Asserted-Identity: <sip:a@example.com>, <sips:b@example.com>` | syntax error: two SIP-family values |
| IH-10 | two separate `tel:` rows | syntax error: two tel values |
| IH-11 | one SIP row plus a second row containing tel and SIP | syntax error: three values across mixed encodings |
| IH-12 | `P-Asserted-Identity: <sip:a%GG@example.com>` | the common URI parser's `HeaderError::Uri` with `UriError::PercentEscape` |
| IH-13 | `P-Asserted-Identity: "Alice <sip:alice@example.com>` | the common address parser's `HeaderError::UnterminatedQuotedString` |
| IH-14 | `P-Asserted-Identity: sip:alice@example.com;user=phone` | one SIP value; `user=phone` remains a URI parameter |
| IH-15 | `P-Asserted-Identity: <sip:alice@example.com>;tag=x` | syntax error: RFC 3325 has no header-parameter tail |
| IH-16 | mixed-case field names for both types | resolved to their recognized `HeaderName` variants |
| IH-17 | construct either type from `<mailto:alice@example.com>` | syntax error; no value is constructed |
| IH-18 | construct either type from `<sip:alice@example.com>;tag=x` | syntax error; no value is constructed and no parameter is silently dropped |
