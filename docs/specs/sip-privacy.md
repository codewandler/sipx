# Typed SIP Privacy header

This document is the normative contract for decoding, constructing and serializing the SIP
`Privacy` header in `sipx-sip`. It specifies syntax and value invariants. It does not provide a
privacy service or choose a privacy policy.

## 1. Normative references

- RFC 3323 §4.2 defines the `Privacy` header grammar, the original `user`, `header`, `session`,
  `none` and `critical` values, the at-most-once rule, `none` exclusivity, and the placement of
  `critical` after requested privacy services. Verified RFC Erratum 5184 corrects its list delimiter
  from semicolon to comma, making repeated rows equivalent to one comma-joined row under RFC 3261
  §7.3.1.
- RFC 3261 §25.1 defines `token`, whitespace and the header-colon grammar used by RFC 3323.
- RFC 3325 §9.3 registers `id` as a Privacy value.
- RFC 7044 §10.1 registers `history` as a Privacy value.

The IANA SIP Privacy Header Field Values registry currently contains `user`, `header`, `session`,
`none`, `critical`, `id`, and `history`. The wire grammar deliberately admits later registered
tokens without requiring a new parser release.

## 2. Types

```text
PrivacyValue = User | Header | Session | None | Critical | Id | History | Extension(bytes)
Privacy      = one checked PrivacyValue
PrivacyList  = non-empty ordered list of Privacy
```

The seven registered spellings decode case-insensitively to their enum variants. `Extension`
contains the exact token octets received or supplied by the caller. This lets policy identify an
extension without losing its spelling while keeping registered comparisons unambiguous.

The order is retained because it is a fact about the received field. It is also the serialization
order; no sort or deduplication silently rewrites an application-provided preference.

`Privacy` represents one list element so the common `TypedHeader` list API can preserve the
equivalence of comma-joined and repeated rows. `Headers::typed_all::<Privacy>()` is the operation
that returns a complete validated field. `PrivacyList` is its checked construction counterpart.

## 3. Decoding

The field is one or more comma-delimited `priv-value` tokens. Optional SP or HTAB adjacent to a
token or comma is ignored. A semicolon is not a Privacy delimiter or token octet. Every repeated
row is appended in wire order before the list invariants are checked.

Decoding returns `HeaderError::Syntax { header: "Privacy" }` when any of these holds:

1. the complete message-wide list is empty or contains an empty segment;
2. a segment contains an octet outside RFC 3261 `token`;
3. two values anywhere across comma-joined or repeated rows compare equal under ASCII
   case-insensitive comparison;
4. `none` occurs with any other value;
5. `critical` is the first value, occurs anywhere except the final position, or follows no privacy
   service.

Every registered or extension value other than `none` and `critical` is a privacy service for rule
5. Treating registered extensions this way lets a later service request be made critical without
teaching an older parser its policy semantics.

RFC 3323 §4.2 says an intermediary must not modify a header containing `none`; preserving the
original `Header` bytes remains the message layer's responsibility. Typed serialization is used
only after an application deliberately constructs or replaces the value.

## 4. Checked construction

`Privacy::new` accepts one `PrivacyValue` and establishes its token invariant. `PrivacyList::new`
accepts an ordered sequence, constructs every element, and applies exactly the message-wide decoding
invariants. An `Extension` must be a non-empty RFC 3261 token and must not case-insensitively spell a
known registered value; callers use the corresponding enum variant for those values. A failed list
construction returns `HeaderError::Syntax` and produces no partially valid `PrivacyList`.

## 5. Serialization

`Privacy::to_bytes` emits one element. `PrivacyList::to_bytes` emits elements in stored order
separated by one `,` and no whitespace. Registered values use their lowercase registry spellings.
Extension octets are emitted unchanged. The same typed value therefore always produces the same
bytes.

## 6. Byte-level vectors

The following table is normative. `ok` names the stored values and exact serialized bytes. `error`
means `HeaderError::Syntax { header: "Privacy" }`.

| id | header rows, in wire order | result |
| --- | --- | --- |
| P1 | `none` | ok `[None]` → `none` |
| P2 | `user,header,session,critical` | ok `[User, Header, Session, Critical]` → same bytes |
| P3 | ` ID , history , VendorX , CRITICAL ` | ok `[Id, History, Extension("VendorX"), Critical]` → `id,history,VendorX,critical` |
| P4 | `user,User` | error: duplicate value in one row |
| P5 | `none,id` | error: same-row `none` conflict |
| P6 | `critical` | error: no requested service |
| P7 | `critical,header` | error: `critical` is not final |
| P8 | `header,critical,session` | error: `critical` is not final |
| P9 | `header,,session` | error: empty segment |
| P10 | `header;session` | error: semicolon is not a Privacy delimiter or token octet |
| P11 | `header,bad=value` | error: `=` is not a token octet |
| P12 | empty value | error: the list is non-empty |
| P13 | `none` then `history` | error: repeated-row `none` conflict |
| P14 | `user` then `User` | error: duplicate across repeated rows |
| P15 | `id` then `critical` | ok `[Id, Critical]` → `id,critical`; list order spans rows |
| P16 | request rows `id` then `history,critical` | `HistoryInfo::apply_message_privacy` anonymizes every entry after consuming the validated typed list |
| P17 | `none` then `bad=value` then `history` | the complete constrained field yields one error and no unvalidated neighboring value |

Construction vectors repeat P2, P3, P4 and P5 using `PrivacyList::new` with typed `PrivacyValue`
inputs. P3 additionally proves that an extension's spelling survives construction and
serialization.
