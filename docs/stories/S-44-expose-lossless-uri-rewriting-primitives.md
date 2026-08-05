---
id: S-44
title: Expose lossless SIP URI user replacement
pillar: Signalling
status: in-progress
priority: 3
design: docs/specs/uri-rewriting.md
epic: sip-core
areas: [sipx-sip, uri]
predicate:
announcement:
note: downstream routing needs parser-owned, byte-exact SIP and SIPS user mutation
---

# Expose lossless SIP URI user replacement

## Goal

Let protocol consumers replace a SIP or SIPS URI user part without rebuilding URI grammar or
changing any byte outside the parser-owned user span.

## Acceptance

- [ ] A normative spec cites RFC 3261 §§19.1.1 and 25.1 and RFC 3986 §2.1, defines the public
      byte-oriented API and records byte-level success, refusal and malformed-input vectors before
      code.
- [ ] `Uri` can replace a SIP or SIPS user part while retaining its password, host, port, URI
      parameters and URI headers byte-for-byte; a successful mutation replaces only the
      parser-retained user span and invalidates the stale verbatim form.
- [ ] Empty, malformed-percent and grammar-breaking replacement values return typed errors without
      partially mutating the URI, and opaque schemes are left byte-exactly untouched.
- [ ] Public integration tests derive from every normative `UR-U` vector, and focused formatting,
      clippy, unit, feature-off and documentation checks pass.

## Progress

- 2026-08-05: Filed from the protocol-generic routing gaps identified downstream. The normative
  contract and vector table were written before the public API or its tests. The filing initially
  combined SIP-user and TEL-subscriber seams; S-48 now owns the distinct RFC 3966 work while both
  stories retain the shared URI-rewriting contract.
- 2026-08-05: The first public-vector run failed on the absent `replace_user`, `tel_parts`,
  `TelUriParts` and `UriError::User` surfaces. That joint run now passes all twelve original public
  vectors, the existing URI-focused tests and all-target/all-feature clippy. The feature-off
  `sipx-sip` build, RFC registry check and complete documentation-site build also pass. Story
  completion and the full workspace gate remain coordinator work; the TEL surfaces are accounted
  for by S-48 rather than this story's narrowed acceptance.
- 2026-08-05: Downstream's N26 contract tightened “retains” to byte identity. The SIP parser now
  records its exact user span; replacement splices only that span and updates it across repeated
  length changes. No mutation path re-scans a delimiter. Adversarial vectors retain mixed-case
  scheme/host syntax, expanded IPv6, password, leading-zero port spelling, parameters and URI
  headers exactly.
- 2026-08-05: Adversarial review found that a read-only TEL split still left downstream rebuilding
  RFC 3966 delimiters and losing mixed-case scheme spelling. The shared contract therefore gained a
  lossless TEL subscriber mutation, now owned by S-48. The same review tightened this story by
  rejecting zero-length SIP userinfo at parse time and stating the actual rewritten-buffer
  allocation bound. Request-line and enclosing address-header span mutation are separate
  protocol-generic follow-up surfaces; this URI-user story does not pretend to provide them. The
  four new failing-first rows now pass with all sixteen public vectors; focused library tests,
  all-feature clippy, feature-off check, rustdoc, RFC report, website sync and link checks are green.

- 2026-08-05: Integration's single full-gate invocation passed repository checks, workspace clippy
  and the complete workspace test suite, then stopped itself before `examples` because the cold
  build exhausted the disk floor. It was an infrastructure non-result and was not rerun.

## Notes

- Considered for the kernel: yes. SIP/SIPS user grammar and the trustworthy mutation span are owned
  by `sipx-sip`; which message fields a routing policy rewrites remains application policy.
