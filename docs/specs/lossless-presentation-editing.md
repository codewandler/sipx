# Lossless address-presentation and Warning-agent editing

## Scope and references

This specification defines two parser-owned edits inside SIP header values:

1. replacing one address presentation — display name, brackets and URI — while retaining the
   address's header-parameter tail; and
2. replacing one complete `Warning` agent with a validated pseudonym while retaining its code and
   quoted text.

Both operations implement RFC 3261 §§7.3.1, 19.1, 20.20, 20.43 and 25.1. Address bracketing also
follows RFC 8217 §§3–4. RFC 5379 §5.1.4 motivates anonymous `From` presentation and §5.1.16
motivates anonymising an identifying Warning hostname, but neither application privacy policy nor
the choice to apply an edit belongs here.

RFC 5379's informational advice to delete the Warning hostname does not amend RFC 3261's grammar:

```text
warning-value = warn-code SP warn-agent SP warn-text
warn-agent    = hostport / pseudonym
pseudonym     = token
```

The agent is mandatory. An agent-less result is malformed, so a privacy consumer replaces the
complete identifying agent with a non-identifying pseudonym such as `anonymous`. The kernel
validates and splices that pseudonym; it cannot decide whether an input agent identifies a UAS.

## Address-presentation operation

The public collection operation is equivalent to:

```rust,ignore
Headers::replace_address_presentation(
    &mut self,
    name: &HeaderName,
    value_index: usize,
    display_name: Option<&str>,
    uri: &Uri,
) -> Result<(), AddressEditError>
```

It recognizes the same single-value and list fields as `replace_address_uri`: `From`, `To`,
`Contact`, `Route`, `Record-Route`, `Path`, `Service-Route`, `P-Asserted-Identity` and
`P-Preferred-Identity`. Indices are zero-based and flattened across comma-joined and repeated rows
in wire order. Every matching row is parsed before mutation, so a malformed later row prevents an
earlier edit.

The shared address parser retains a half-open presentation span. In name-address form it begins at
the display name, or at `<` when no display name exists, and ends after `>`. In bare addr-spec form
it is the parser-owned URI span. The replacement is always unambiguous name-address syntax:

```text
[ quoted-display-name SP ] LAQUOT URI RAQUOT
```

A present display name is valid UTF-8, is always quoted, and escapes `\` and `"`. CR, LF, NUL,
other ASCII controls and DEL are rejected. The URI is serialized and parsed through `Uri`; the
candidate address row is then parsed through the same address grammar before it is committed.

Only that presentation span changes. Header-name spelling, colon whitespace, leading value
whitespace, every byte following the presentation — including whitespace, folding, semicolons,
parameter-name spelling and quoted parameter escapes — list delimiters, other values and other
rows remain byte-identical. Converting a bare address to name-address form therefore does not
rebuild its parameter tail. All failures are atomic.

## Warning-agent operation

The public collection operation is equivalent to:

```rust,ignore
Headers::replace_warning_agent_with_pseudonym(
    &mut self,
    value_index: usize,
    pseudonym: &[u8],
) -> Result<(), WarningEditError>
```

The Warning parser recognizes RFC 3261's comma-separated `warning-value` list, including repeated
rows, quoted-string escapes, commas inside `warn-text`, hostports and pseudonyms. Value indices are
zero-based and flattened across rows in wire order. It retains the complete `warn-agent` span from
the same pass that validates the three-digit code and quoted text.

The replacement must be one non-empty RFC 3261 `token`; it cannot contain separators, whitespace,
controls or line breaks. Only the selected agent span changes. The code, both separator spaces,
quoted text including escapes, folding, comma layout, other values and other rows remain
byte-identical. All Warning rows are valid before any one is changed. A missing agent is malformed,
not an already-anonymised value.

`WarningEditError` distinguishes malformed Warning syntax, a flattened index beyond the complete
field and an invalid replacement pseudonym. All failures leave the complete header collection
unchanged.

## Byte-level vectors

In the exact results below, `CRLF` denotes the two framing bytes and `HTAB` denotes one tab byte.
The public integration tests use those bytes rather than the labels.

### Address presentation

| ID | Input field value | Operation | Exact result |
|---|---|---|---|
| LP-A-1 | `"Anna" <sip:a@old.example>;tag=a1` | index 0, display `Anonymous`, URI `sip:anonymous@anonymous.invalid` | `"Anonymous" <sip:anonymous@anonymous.invalid>;tag=a1` |
| LP-A-2 | `"A\\\" B"<sip:a@old.example> CRLF HTAB ;TaG=a1;note="x\\\";y"` | anonymous presentation | `"Anonymous" <sip:anonymous@anonymous.invalid> CRLF HTAB ;TaG=a1;note="x\\\";y"`; the fold and escaped parameter tail are byte-identical |
| LP-A-3 | `sip:a@old.example ;tag=a1;opaque="a\\b"` | anonymous presentation | `"Anonymous" <sip:anonymous@anonymous.invalid> ;tag=a1;opaque="a\\b"`; bare form becomes name-address without rebuilding the suffix |
| LP-A-4 | `Route: <sip:a@h>, CRLF HTAB "old" <sip:b@h>;x="q\\\"r"` plus a repeated Route row | flattened index 1, display `A "B\C`, URI `sips:b@n` | only value 1's presentation becomes `"A \"B\\C" <sips:b@n>`; fold, parameter, delimiters and repeated row are exact |
| LP-A-5 | valid first `From` row followed by a malformed repeated `From` row | replace index 0 | `Malformed`; both rows unchanged |
| LP-A-6 | `From: "unterminated <sip:a@h>;tag=x` | replace index 0 | `Malformed`; field unchanged |
| LP-A-7 | valid `From` | display name containing CRLF, NUL or DEL | typed refusal; field unchanged |
| LP-A-8 | valid `From` | URI whose serialized form makes the enclosing candidate invalid | `Malformed` or the existing typed URI refusal; field unchanged |

### Warning agent

| ID | Input field value | Operation | Exact result |
|---|---|---|---|
| LP-W-1 | `399 pbx.acme.example "Media downgraded"` | index 0, pseudonym `anonymous` | `399 anonymous "Media downgraded"` |
| LP-W-2 | `399 pbx.example:5060 CRLF HTAB "Media \\"downgraded\\""` | index 0, pseudonym `anonymous` | only `pbx.example:5060` changes; the fold and quoted text are byte-identical |
| LP-W-3 | `399 old.example "comma, and \\"quote\\"", 301 [2001:db8::1]:5060 "Second"` plus a repeated Warning row | flattened index 1, pseudonym `anonymous` | only the IPv6 hostport changes; comma, escaped text and repeated row are exact |
| LP-W-4 | `399 anonymous "already private"` | index 0, pseudonym `anonymous` | byte-identical success |
| LP-W-5 | `399 "missing agent"` | index 0 | `Malformed`; field unchanged |
| LP-W-6 | `39A old.example "bad code"`, unterminated text, or malformed later row | replace an otherwise valid value | `Malformed`; every row unchanged |
| LP-W-7 | valid Warning | empty, `not anonymous`, comma, CRLF, NUL or non-token replacement | `InvalidPseudonym`; every row unchanged |
| LP-W-8 | one Warning value | index 1 | `IndexOutOfRange`; field unchanged |

## Security and ownership

Neither operation searches raw bytes for a delimiter or reconstructs syntax outside its retained
span. Size arithmetic is checked, candidate parsing precedes assignment, and hostile input cannot
panic the process. Both operations are pure message transformations with no clock, randomness or
I/O.

Considered for the application layer: no. SIP address presentation, Warning list syntax, quoting,
escaping and trustworthy source ranges are protocol-generic and belong to `sipx-sip`. Whether a
privacy or routing policy invokes either operation remains consumer-owned.
