---
id: S-44
title: Expose lossless URI rewriting primitives
pillar: Signalling
status: in-progress
priority: 3
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip, uri]
predicate:
announcement:
note: downstream routing needs protocol-owned SIP user mutation and structured tel access
---

# Expose lossless URI rewriting primitives

## Goal

Let protocol consumers replace a SIP or SIPS URI user part and a `tel:` telephone-subscriber
without rebuilding URI grammar, and expose the subscriber and parameter tail without duplicating
RFC 3966 delimiter handling.

## Acceptance

- [ ] A normative spec cites RFC 3261 §19.1.1 and RFC 3966 §§3–4, defines the public byte-oriented
      APIs and records byte-level success, refusal and malformed-input vectors before code.
- [ ] `Uri` can replace a SIP or SIPS user part while retaining its password, host, port, URI
      parameters and URI headers byte-for-byte; a successful mutation replaces only the
      parser-retained user span and invalidates the stale verbatim form.
- [ ] Empty, malformed-percent and grammar-breaking replacement values return typed errors without
      partially mutating the URI, and opaque schemes are left byte-exactly untouched.
- [ ] A public typed view splits a `tel:` URI into its exact telephone-subscriber and optional raw
      parameter tail, preserving the distinction between no separator and an empty tail.
- [ ] `Uri` can replace a validated RFC 3966 telephone-subscriber through its parser-retained span,
      preserving mixed-case scheme spelling and the complete optional parameter tail byte-for-byte.
- [ ] Public integration tests derive from every normative vector, and focused formatting, clippy,
      unit, feature-off and documentation checks pass.

## Progress

- 2026-08-05: Filed from the protocol-generic routing gaps identified downstream. The normative
  contract and vector table were written before the public API or its tests.
- 2026-08-05: The first public-vector run failed on the absent `replace_user`, `tel_parts`,
  `TelUriParts` and `UriError::User` surfaces. The implementation now passes all twelve public vectors,
  the existing URI-focused tests and all-target/all-feature clippy. The feature-off `sipx-sip`
  build, RFC registry check and complete documentation-site build also pass. Story completion and
  the full workspace gate remain coordinator work.
- 2026-08-05: Downstream's N26 contract tightened “retains” to byte identity. The SIP parser now
  records its exact user span; replacement splices only that span and updates it across repeated
  length changes. No mutation path re-scans a delimiter. Adversarial vectors retain mixed-case
  scheme/host syntax, expanded IPv6, password, leading-zero port spelling, parameters and URI
  headers exactly.
- 2026-08-05: Adversarial review found that a read-only TEL split still left downstream rebuilding
  RFC 3966 delimiters and losing mixed-case scheme spelling. The contract now owns a lossless TEL
  subscriber mutation too, rejects zero-length SIP userinfo at parse time, and states the actual
  rewritten-buffer allocation bound. Request-line and enclosing address-header span mutation are
  separate protocol-generic follow-up surfaces; this URI story does not pretend to provide them.

## Notes

- This belongs in `sipx-sip`: both operations expose URI grammar already owned by the kernel. Which
  message fields a routing policy rewrites remains application policy.
- The existing `Uri::equivalent` implementation already contains the RFC 3966 split privately;
  S-44 exposes and mutates at that parse boundary rather than introducing a second algorithm.
