# Spec: URI rewriting primitives

**Status:** normative · **Stories:** S-44, S-48, S-49 · **Crate:** `sipx-sip` · **Design:**
[sip-core](../designs/sip-core.md)

This contract defines byte-oriented URI seams for consumers that must inspect or change a number
without implementing SIP or `tel:` URI grammar themselves.

## 1. Normative references and boundary

- RFC 3261 §19.1.1 defines SIP/SIPS `userinfo`, including the `user`, optional password and the
  delimiter sets that distinguish them from the host, parameters and headers. Section 25.1 defines
  `user = 1*( unreserved / escaped / user-unreserved )`.
- RFC 3986 §2.1 defines percent encoding as `%` followed by exactly two hexadecimal digits.
- RFC 3966 §3 defines a `telephone-subscriber` followed by zero or more `;`-introduced parameters,
  `pname` as one or more alphanumeric or `-` bytes, and a non-empty `pvalue` from its `paramchar`
  production. Section 4 makes parameter-name comparison case-insensitive and identifies visual
  separators as subscriber syntax rather than parameter delimiters.

The API owns syntax, not routing policy. It never normalises digits, chooses a source identity or
decides whether a `phone-context` value is suitable. It does expose each syntactically valid generic
TEL parameter and owns the case-insensitive comparison of its name. All input and output are bytes
because valid percent escapes can decode to NUL or non-UTF-8.

## 2. Public types

```rust
impl Uri {
    pub fn replace_user(
        &mut self,
        user: impl Into<bytes::Bytes>,
    ) -> Result<bool, UriError>;

    pub fn replace_tel_subscriber(
        &mut self,
        subscriber: impl Into<bytes::Bytes>,
    ) -> Result<bool, UriError>;

    pub fn tel_parts(&self) -> Option<TelUriParts<'_>>;
}

pub struct TelUriParts<'a> { /* borrowed exact spans */ }

impl<'a> TelUriParts<'a> {
    pub fn subscriber(&self) -> &'a [u8];
    pub fn parameters(&self) -> Option<&'a [u8]>;
    pub fn parsed_parameters(&self) -> TelParameters<'a>;
}

pub struct TelParameters<'a> { /* allocation-free iterator over the retained tail */ }

impl<'a> Iterator for TelParameters<'a> {
    type Item = Result<TelParameter<'a>, TelParameterError>;
}

pub struct TelParameter<'a> { /* borrowed exact name and optional value */ }

impl<'a> TelParameter<'a> {
    pub fn name(&self) -> &'a [u8];
    pub fn value(&self) -> Option<&'a [u8]>;
    pub fn name_eq(&self, expected: &[u8]) -> bool;
}

pub struct TelParameterError { /* offending tail-relative byte offset and kind */ }

impl TelParameterError {
    pub fn offset(&self) -> usize;
    pub fn kind(&self) -> TelParameterErrorKind;
}

#[non_exhaustive]
pub enum TelParameterErrorKind {
    Empty,
    Name,
    Value,
}
```

`replace_user` takes an already percent-encoded user part. `Ok(true)` means an existing SIP or SIPS
user was changed. `Ok(false)` means the scheme has no SIP user part or the SIP/SIPS URI has no
userinfo; the URI is untouched. Invalid input on a SIP/SIPS URI with a user is `UriError::User` or
`UriError::PercentEscape` and is atomic.

`replace_tel_subscriber` takes an RFC 3966 `global-number-digits` or `local-number-digits` value,
including any visual separators but excluding URI parameters. `Ok(true)` means a `tel:` subscriber
was changed, while every other scheme returns `Ok(false)` unchanged. An empty value or one outside
the subscriber productions is `UriError::TelephoneSubscriber` and is atomic. The method validates
the subscriber production only: it does not interpret the retained parameters or decide whether a
local number's `phone-context` is suitable.

`tel_parts` is present only for `Scheme::Tel`. `subscriber` is the exact body prefix before the
first `;`. `parameters` is the exact suffix after that delimiter, without the delimiter itself.
It is `None` when there is no delimiter and `Some(b"")` when the body ends in `;`; retaining that
distinction makes the view lossless even though the latter is not a valid RFC 3966 parameter.
`Uri::parse` validates the subscriber production but deliberately retains the parameter tail
without interpreting or validating it. It also validates percent-escape shape in opaque URI bodies;
the remainder of an unknown scheme stays opaque rather than acquiring scheme-specific semantics.

`parsed_parameters` is an allocation-free iterator over the same retained tail. No tail produces
no items. Each successful item borrows an exact `pname` and an optional exact `pvalue`; a missing
`=` is distinct from `=` followed by an invalid empty value. `name_eq` compares a valid caller name
to the parsed name with ASCII case folding, as RFC 3966 §4 requires. It does not decode or
canonicalise either spelling.

The iterator preserves input order and duplicates. It performs structural validation only: it does
not classify `ext`, `isub` or `phone-context`, enforce their uniqueness or select a context for a
local number. A consumer can therefore count every case-insensitive `phone-context` occurrence and
inspect whether it carried a value without re-parsing delimiters. Empty segments, an empty or
illegal `pname`, an empty `pvalue`, or a byte outside `pvalue` produces `TelParameterError` at the
tail-relative start of the offending component. The iterator emits that error once and is then
fused, so a malformed suffix cannot be mistaken for a complete parameter set. A percent-encoded
`;` or `=` is part of the exact value and never becomes a delimiter.

## 3. Mutation contract

The replacement is accepted only when it is non-empty and every byte is an RFC 3261 `user` byte:
ASCII alphanumeric, `-_.!~*'()`, `&=+$,;?/`, or a well-formed percent escape. A literal `:`, `@`,
whitespace, non-ASCII byte, control byte, or malformed escape is rejected.

An empty userinfo is not an RFC 3261 user: `sip:@example.com` and
`sip::password@example.com` both fail parsing with `UriError::User` rather than creating a
zero-length mutation span.

A successful SIP replacement changes only the structured `user` field. For a parsed URI it splices
the replacement into the user byte span retained by that same parse. Every byte outside that span is
identical: scheme spelling, password, `@` and `:` delimiters, host case, expanded IPv6 spelling,
port spelling, URI-parameter order/spelling and URI-header order/spelling. The old verbatim form is
invalidated by replacing it with this rewritten wire form, so serialization cannot replay the stale
user. There is no delimiter scan in the mutation path: the parser records the span while it already
separates userinfo.

If another structured mutation has already discarded verbatim bytes, replacement updates the user
and the existing canonical serializer writes the URI. A URI with no user is not changed: inserting
userinfo would also insert an `@` delimiter outside any existing user position, violating this
operation's lossless contract.

A TEL replacement accepts either `+` followed by digits and RFC 3966 visual separators, with at
least one digit, or a local subscriber made from hexadecimal digits, `*`, `#` and visual separators,
with at least one hexadecimal digit, `*` or `#`. It splices only the parser-retained subscriber
span, updates that span across repeated length changes, and retains the original scheme spelling
plus the entire optional `;` parameter tail byte-for-byte. The parameter delimiter is never
reconstructed by the caller or rescanned in the mutation path.

## 4. State table

| Scheme/state | Replacement | Result | State change |
|---|---|---|---|
| SIP or SIPS with user | valid non-empty user | `Ok(true)` | replace only parser-owned user span; invalidate stale verbatim form |
| SIP or SIPS with user | empty or illegal user byte | `Err(UriError::User)` | none |
| SIP or SIPS with user | malformed percent escape | `Err(UriError::PercentEscape)` | none |
| SIP or SIPS without user | any bytes | `Ok(false)` | none, including verbatim form |
| `tel:` through `replace_user` | any bytes | `Ok(false)` | none, including verbatim form |
| `tel:` through `replace_tel_subscriber` | valid subscriber | `Ok(true)` | replace only parser-owned subscriber span |
| `tel:` through `replace_tel_subscriber` | empty or illegal subscriber | `Err(UriError::TelephoneSubscriber)` | none |
| non-`tel:` through `replace_tel_subscriber` | any bytes | `Ok(false)` | none, including verbatim form |
| another opaque scheme through `replace_user` | any bytes | `Ok(false)` | none, including verbatim form |

Neither operation reads a clock or performs I/O. A successful mutation allocates one rewritten URI
buffer proportional to the original URI plus the replacement; validation failures and no-op scheme
refusals do not rewrite it. `tel_parts` borrows the existing URI body and allocates nothing.

## 5. Byte-level vectors

The vector IDs are test names' contract prefixes.

| ID | Operation and input | Expected result |
|---|---|---|
| UR-U-1 | replace with `new%2Buser` on `sip:old:secret@example.com:5070;transport=tcp?subject=x` | `Ok(true)` and `sip:new%2Buser:secret@example.com:5070;transport=tcp?subject=x` |
| UR-U-2 | replace with `7042;isub=9?x/y` on `sips:old@example.com` | `Ok(true)`; every RFC 3261 user delimiter remains inside the user |
| UR-U-3 | replace with empty, `a@b`, `a:b`, a space, CRLF or byte `ff` on an uppercase parsed SIP URI | `UriError::User`; original bytes remain exact |
| UR-U-4 | replace with `bad%2`, `bad%xx` or `bad%` on a parsed SIPS URI | `UriError::PercentEscape`; original bytes remain exact |
| UR-U-5 | replace with arbitrary invalid bytes on `TEL:+1-201-555-0123;ext=9` | `Ok(false)` and the uppercase-scheme bytes remain exact |
| UR-U-6 | replace twice on `SiPs:old:p%61ss@[2001:0DB8:0:0:0:0:0:1]:05061;Transport=TCP;foo=%2f?Subject=X&x=%2F`, first with longer `n%65w`, then shorter `x` | only `old` changes each time; mixed-case scheme, password, expanded IPv6, five-digit port spelling, parameter/header case, order and escapes remain byte-identical |
| UR-U-7 | replace on `SIP:ExAmPlE.COM:05060;Transport=UDP?Subject=X`, which has no userinfo | `Ok(false)` and all original bytes remain exact |
| UR-U-8 | append `lr` through the general mutation API, then replace `old` with `new` on `SIP:old@[2001:0DB8:0:0:0:0:0:1]` | `sip:new@[2001:db8::1];lr`; the already non-verbatim URI uses structured serialization |
| UR-U-9 | parse `sip:@example.com` or `sip::password@example.com` | `UriError::User`; no zero-length user span is created |
| UR-T-1 | split `tel:+1-201-555-0123;ext=9;Phone-Context=+1-201` | subscriber `+1-201-555-0123`; parameters `Some(ext=9;Phone-Context=+1-201)` byte-exactly |
| UR-T-2 | split `TEL:7042` | subscriber `7042`; parameters `None` |
| UR-T-3 | split `tel:7042;` | subscriber `7042`; parameters `Some(b"")` |
| UR-T-4 | split `sip:7042@example.com;user=phone` and `urn:7042;ext=9` | `None` for both |
| UR-T-5 | replace three times on `TeL:+1-(201)-555-0123;Ext=9;Phone-Context=+1-201`, with longer global `+49-30-123456`, shorter local `7042`, then the dial-symbol-only local subscriber `*#` | only the subscriber changes each time; mixed-case scheme and the complete parameter tail remain byte-identical |
| UR-T-6 | replace with empty, `+`, `+12A`, `12G`, `12:34`, whitespace, CRLF or byte `ff` on a parsed TEL URI | `UriError::TelephoneSubscriber`; original bytes remain exact |
| UR-T-7 | replace with arbitrary invalid bytes on a SIP URI and an unknown opaque scheme | `Ok(false)` and both URIs remain byte-exact without validating a TEL subscriber |
| UR-T-8 | parse `tel:` or `tel:+` | `UriError::TelephoneSubscriber`; no malformed TEL URI is constructed |
| UR-P-1 | iterate `TEL:7042` | no items |
| UR-P-2 | iterate `tel:7042;phone-context=example.com` | one exact `phone-context=example.com` item; `name_eq(b"PHONE-CONTEXT")` is true |
| UR-P-3 | iterate `tel:7042;ext=9` | one exact `ext=9` item and no `phone-context` match |
| UR-P-4 | iterate `tel:7042;foo=x;phone-context=example.com;ext=9` | three exact items in wire order |
| UR-P-5 | iterate `tel:7042;PhOnE-CoNtExT=example.com` | the original name spelling is retained and `name_eq(b"phone-context")` is true |
| UR-P-6 | iterate `tel:7042;foo=a%3Bb%3Dc` | one value `a%3Bb%3Dc`; escaped delimiters do not split it |
| UR-P-7 | iterate `tel:7042;foo=one;FOO=two` | two separate items, both matching `foo`, in wire order |
| UR-P-8 | iterate tails `;`, `;;ext=9`, `;=x`, `;foo=`, `;foo?=x` | one typed `Empty`, `Name` or `Value` error at the offending tail-relative offset, then end of iteration |
| UR-O-1 | parse `mailto:%GG` and `mailto:alice%40example.com` | respectively `UriError::PercentEscape` and a byte-exact opaque URI |

## 6. Change rule

Adding an accepted user, TEL subscriber, parameter-name or parameter-value byte; changing the
atomicity, span, parameter-error or invalidation rules; collapsing the TEL tail's `None`/empty
distinction; coalescing duplicate parameters; or interpreting TEL parameter semantics requires a
spec and vector change before code.
