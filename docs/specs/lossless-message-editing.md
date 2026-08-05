# Lossless nested URI editing

## Scope and references

This specification defines parser-owned editing of a URI nested in a SIP request line or an
address-valued header. It implements the message syntax and list equivalence in RFC 3261
§§7.1, 7.3.1, 19.1 and 25.1. It does not define routing, identity trust or number-normalisation
policy.

The operation is deliberately grammatical. Searching a field for the URI's bytes is incorrect:
in `To: "sip:old@example.test" <sip:old@example.test>` the first equal byte string is display
text and the second is the URI. Only the address parser may identify the latter.

## Model

A parsed request retains the exact start line and the parser-owned half-open byte span of its
Request-URI. Replacing the URI splices that span and retains the method spelling, both separator
spaces and SIP-version bytes. A constructed request has no retained span and is serialized in the
ordinary deterministic form.

An address field edit operates on these recognized grammars:

| Field | Row grammar |
|---|---|
| `From`, `To` | one address |
| `Contact`, `Route`, `Record-Route`, `Path`, `Service-Route` | address list |
| `P-Asserted-Identity`, `P-Preferred-Identity` | address list |

Value indices are zero-based and flattened over repeated rows in wire order. The same index selects
the same value for replacement or removal. Unsupported field names, malformed address rows and an
index beyond the flattened list are typed errors. A `Contact: *` row is not an address row and is
therefore malformed for this operation.

Replacement splices only the parser-owned URI span. Field-name spelling, whitespace around the
colon, folding outside the URI, display name, angle brackets, header parameters, delimiters and
every other value remain byte-identical. A replacement URI is serialized and parsed again before
the splice; this enforces the URI grammar and prevents CR, LF or NUL from entering a message. The
candidate row is then parsed again through the same address layout, and is committed only when its
value count is unchanged and the selected parser-owned URI bytes equal the complete replacement.
Thus a standalone-valid URI cannot introduce a bare-address delimiter that the enclosing grammar
would reinterpret.

Removal deletes the selected value and exactly one adjacent list separator when another value
remains. For a non-final value it removes the value, the following comma and following linear
whitespace. For the final value it removes the preceding comma and preceding linear whitespace.
Removing a row's sole value removes that header row. Other rows and values remain byte-identical.

Folding is unfolded with a source-byte map for grammar parsing, then edits are projected back onto
the original bytes. A fold between address tokens or beside a comma is therefore retained unless it
belongs to a removed separator region. A fold inside a URI unfolds to whitespace, which the URI
grammar rejects; such a field is malformed rather than approximately edited.

## Operations

| Input | Result |
|---|---|
| parsed request and valid URI | replace retained Request-URI span |
| constructed request and valid URI | replace typed URI; deterministic serialization |
| supported address field, valid flattened index and URI | replace only selected URI span |
| supported address field and valid flattened index | remove selected value; remove row if empty |
| unsupported field | `UnsupportedHeader` |
| malformed address/list | `Malformed` with the shared header error |
| index past the flattened value list | `IndexOutOfRange` |
| replacement URI whose serialization is invalid | `InvalidUri` |

All failures are atomic.

## Byte-level vectors

| ID | Operation | Input | Replacement / index | Exact result |
|---|---|---|---|---|
| LM-1 | request replace | `iNvItE sip:old@EXAMPLE.test SiP/2.0` | `sips:new@example.net` | `iNvItE sips:new@example.net SiP/2.0` |
| LM-2 | request replace twice | LM-1 result | `tel:+12025550123` | `iNvItE tel:+12025550123 SiP/2.0` |
| LM-3 | built request replace | constructed `OPTIONS sip:a@b SIP/2.0` | `sip:c@d` | `OPTIONS sip:c@d SIP/2.0` |
| LM-4 | ambiguous display text | `t : "sip:old@h" <sip:old@h>;tag=x` | index 0, `sip:new@h` | `t : "sip:old@h" <sip:new@h>;tag=x` |
| LM-5 | folded address | `To:\tAlice\r\n \t<sip:old@h> ; tag=x` | index 0, `sip:new@h` | only `sip:old@h` changes |
| LM-6 | comma list | `Route: <sip:a@h>,  <sip:b@h>,<sip:c@h>` | index 1, `sips:b@n` | only the second URI changes |
| LM-7 | repeated rows | two `P-Asserted-Identity` rows, first with two values | flattened index 2 | URI in the second row changes |
| LM-8 | remove middle | `Route: <sip:a@h>,  <sip:b@h>, <sip:c@h>` | index 1 | `Route: <sip:a@h>,  <sip:c@h>` |
| LM-9 | remove last | LM-8 result | index 1 | `Route: <sip:a@h>` |
| LM-10 | remove sole row | two repeated identity rows | flattened index 0 | first row is absent; second is exact |
| LM-11 | malformed | `To: not an address` | index 0 | typed `Malformed`, bytes unchanged |
| LM-12 | unsupported | `Subject: sip:a@h` | index 0 | typed `UnsupportedHeader`, bytes unchanged |
| LM-13 | out of range | `From: <sip:a@h>` | index 1 | typed `IndexOutOfRange`, bytes unchanged |
| LM-14 | folded separator removal | `Route: <sip:a@h>,\r\n \t<sip:b@h>` | index 0 | `Route: <sip:b@h>` |
| LM-15 | bare delimiter refusal | bare `To` with `;` or `?`, bare `Contact` with `,` | index 0, standalone-valid URI | typed `Malformed`, bytes unchanged |
| LM-16 | delimited URI acceptance | corresponding `To` / `Contact` name-address forms | index 0, same URIs as LM-15 | complete replacement appears exactly inside `<>` |
| LM-17 | later malformed row | valid first `Route`, malformed second `Route` | flattened index 0 | typed `Malformed`, both rows unchanged |
| LM-18 | remove final with trailing fold | `Route: <sip:a@h>, <sip:b@h>\t \r\n \t` | index 1 | `Route: <sip:a@h>\t \r\n \t` |

## Security and ownership

The message and address parsers own all byte ranges. No edit locates a URI through byte search, and
no network input is indexed without a checked access. The operations are sans-I/O and contain no
policy. This belongs in `sipx-sip` because request-line syntax, address grammar and RFC 3261 list
equivalence are protocol-generic.
