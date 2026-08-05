---
id: S-46
title: Type the Privacy header
pillar: Signalling
status: in-progress
priority: 3
design: docs/specs/sip-privacy.md
epic: conformance
areas: [sipx-sip, privacy, routing]
predicate:
announcement:
note: RFC 3323 typed grammar needed by privacy and asserted-identity policy
---

# Type the Privacy header

## Goal

Expose `Privacy` as one validated typed value so policy code can act on registered privacy requests
without reparsing delimiters, while retaining future extension tokens.

## Acceptance

- [ ] A normative spec cites RFC 3323 §4.2 and defines the registered values, extension-token
      representation, construction invariants, deterministic serialization, and byte vectors.
- [ ] `sipx-sip` exposes a typed `Privacy` header whose registered values are enum variants and whose
      extension value retains its token spelling for application policy.
- [ ] Decoding rejects empty or non-token values, duplicates, `none` mixed with another value,
      `critical` before another value, and `critical` without a requested privacy service with a
      typed `HeaderError`, never a panic.
- [ ] The checked list constructor enforces the same message-wide invariants as wire decoding, and
      serialization emits one deterministic comma-delimited value per verified RFC Erratum 5184.
- [ ] Failing-first integration tests derive from every normative vector and prove access through
      `Headers::typed_all`; focused formatting, clippy, SIP tests, feature-off build and docs checks
      pass.

## Progress

- 2026-08-05: wrote the normative RFC 3323 grammar, invariants and byte-vector table before adding
  the typed implementation.
- 2026-08-05: the first vector run failed on the deliberately absent `Privacy` and `PrivacyValue`
  API. The implemented types now recognize all seven registered values, retain extension spelling,
  enforce comma-list invariants across joined and repeated rows through one validator, and make
  History-Info consume the typed values instead of splitting delimiters itself. The delimiter
  follows verified RFC Erratum 5184 rather than the erroneous semicolon in the published ABNF.
- 2026-08-05: focused format, all-target/all-feature clippy, all SIP tests, no-default-features
  checking, RFC/maturity/comparison/website consistency, internal links and the public site build
  pass. The story remains in progress for integration review and the repository-wide gate.

## Notes

- `HeaderName::Privacy` already exists. This story types its value; it does not implement a privacy
  service or decide when a UA should request privacy.
- RFC 3323 owns the generic grammar. RFC 3325 registers `id`, and RFC 7044 registers `history`; both
  are represented as known values. Later token registrations remain visible as extensions.
