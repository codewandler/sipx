---
id: S-45
title: Type RFC 3325 asserted and preferred identity lists
pillar: Signalling
status: done
priority: 3
design: docs/specs/sip-asserted-identity.md
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
      preserve the address parser's typed errors for malformed input, and enforce RFC 8217's
      name-address requirement for URI delimiters.
- [x] `Headers::typed_all` treats comma-joined values and repeated rows identically while enforcing
      strict RFC 3325 construction diagnostics across the whole field without making ordinary
      unconstrained typed headers eager.
- [x] Dedicated receive-list APIs implement RFC 5876 §4.5 across all rows: valid values survive,
      and every ignored scheme/count/combination is reported with a stable index and reason so a
      proxy can satisfy the MUST NOT forward obligation.
- [x] Checked complete-list constructors enforce strict sending cardinality and pairing;
      deterministic serialization preserves value order and emits an unambiguous name-address form.
- [x] The normative spec precedes the implementation and byte-level tests cover both headers,
      both list encodings, each permitted scheme, RFC 5876 filtering, RFC 8217 delimiter cases,
      invalid construction and generic address/URI failures.
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
- 2026-08-05: adversarial review found that the first pass treated RFC 3325's sending shape as a
  receive-time rejection, contrary to RFC 5876 §4.5, and accepted a bare URI parameter contrary to
  RFC 8217. The receive-list API now preserves valid identities while reporting every ignored value
  by flattened wire-order index and reason; strict checked list construction remains separate. The
  RFC 8217-invalid parser path was removed, manually assembled display names are validated, and
  message-wide validation is opt-in so existing `typed_all` users remain lazy.
- 2026-08-05: final cross-wave review found three shared-grammar bypasses: broad ASCII trimming made
  receive filtering accept VT/FF that lossless removal rejected, the RFC 8217 question-mark check
  only recognized structured SIP queries, and opaque TEL/unknown URI bodies admitted malformed
  subscribers or escapes. Failing vectors IH-26 through IH-28 now hold the shared whitespace,
  address and URI boundaries to the receive, construction and removal contracts together.

- 2026-08-05: Integration's single full-gate invocation passed repository checks, workspace clippy
  and the complete workspace test suite, then stopped itself before `examples` because the cold
  build exhausted the disk floor. It was an infrastructure non-result and was not rerun.

- 2026-08-05: the protected beta.7 workflow completed the full repository gate at the immutable
  release tag. Every acceptance item is now satisfied and the story closes with that exact evidence.

## Notes

- RFC 3325 §§9.1–9.2 define the same strict sending grammar for both headers; RFC 5876 §4.5 defines
  tolerant sequential receive filtering, and RFC 8217 supplies the delimiter/bracket rule.
- This belongs in `sipx-sip`: field-name recognition, address parsing and list cardinality are
  protocol grammar, while a routing application remains responsible for trust and assertion policy.
