---
id: S-45
title: Type RFC 3325 asserted and preferred identity lists
pillar: Signalling
status: in-progress
priority: 3
design:
epic:
areas: [sipx-sip, routing]
predicate:
announcement:
note: protocol-generic identity header grammar required by routing consumers
---

# Type RFC 3325 asserted and preferred identity lists

## Goal

Give every SIP consumer one typed, sans-I/O implementation of the RFC 3325
`P-Asserted-Identity` and `P-Preferred-Identity` value-list grammar instead of making each
consumer recognize names, split rows and enforce the scheme pairing itself.

## Acceptance

- [x] `HeaderName` recognizes both field names case-insensitively and classifies them as
      comma-separated lists.
- [x] Typed values reuse the kernel address and URI grammar, accept only `sip`, `sips` and `tel`,
      and preserve the address parser's typed errors for malformed input.
- [x] `Headers::typed_all` treats comma-joined values and repeated rows identically while enforcing
      RFC 3325 §§9.1–9.2 across the whole field: one value, or one SIP/SIPS value paired with one
      tel value.
- [x] Deterministic serialization preserves value order and emits an unambiguous name-address form.
- [x] The normative spec precedes the implementation and byte-level tests cover both headers,
      both list encodings, each permitted scheme, invalid cardinality, invalid pairing, unsupported
      schemes and generic address/URI failures.
- [x] Focused formatting, clippy, tests and documentation checks are green.

## Progress

- 2026-08-05: filed from the routing-syntax boundary audit; the contract and vector table were
  written before the header types.
- 2026-08-05: the new integration vector target failed first on the absent types and names. The
  implementation now adds the two typed address values, field-wide validation through
  `Headers::typed_all`, deterministic name-address serialization and recognized header names.
- 2026-08-05: all 9 identity-vector tests and all 240 `sipx-sip` unit tests pass; package clippy with
  `-D warnings`, the no-default-feature build, formatting, RFC/maturity/comparison reports, generated
  website regions, internal links, the Docusaurus build and `-D warnings` rustdoc are green. The
  complete repository gate remains for integration.
- 2026-08-05: review hardening made the inner address private and added checked constructors, so an
  unsupported scheme or header-parameter tail cannot bypass decode-time invariants or be silently
  dropped by serialization. Two construction vectors cover both refusals.

## Notes

- RFC 3325 §§9.1–9.2 define the same value grammar and one-or-two constraint for both headers.
- This belongs in `sipx-sip`: field-name recognition, address parsing and list cardinality are
  protocol grammar, while a routing application remains responsible for trust and assertion policy.
