---
id: S-48
title: Expose lossless TEL URI subscriber editing
pillar: Signalling
status: done
priority: 3
design: docs/specs/uri-rewriting.md
epic: sip-core
areas: [sipx-sip, uri]
predicate:
announcement:
note: public RFC 3966 split and parser-owned subscriber replacement for routing consumers
---

# Expose lossless TEL URI subscriber editing

## Goal

Let protocol consumers inspect and replace a `tel:` telephone-subscriber without duplicating RFC
3966 delimiter handling or changing the original scheme spelling and parameter tail.

## Acceptance

- [x] The shared normative spec cites RFC 3966 §§3–4, defines the public byte-oriented TEL APIs and
      records byte-level success, refusal and malformed-input vectors before code.
- [x] A public typed view splits a `tel:` URI into its exact telephone-subscriber and optional raw
      parameter tail, preserving the distinction between no separator and an empty tail.
- [x] `Uri` can replace a validated RFC 3966 telephone-subscriber through its parser-retained span,
      preserving mixed-case scheme spelling and the complete optional parameter tail byte-for-byte.
- [x] Global and local subscriber validation accepts only the RFC 3966 productions, returns a typed
      error atomically for invalid input, and leaves non-TEL schemes byte-exactly untouched.
- [x] Public integration tests derive from every normative `UR-T` vector, and focused formatting,
      clippy, unit, feature-off and documentation checks pass.

## Progress

- 2026-08-05: Split from S-44 after adversarial review showed that a read-only TEL view still made
  downstream consumers rebuild RFC 3966 delimiters and lose mixed-case scheme spelling. The shared
  spec already preceded the implementation and owns the seven `UR-T` byte-vector rows.
- 2026-08-05: The original joint failing-first run established that `tel_parts` and `TelUriParts`
  were absent. The implementation now exposes the exact borrowed subscriber and optional parameter
  tail, including the `None` versus empty-tail distinction.
- 2026-08-05: The follow-up failing-first rows established that read-only access was insufficient.
  `replace_tel_subscriber` now validates RFC 3966 global and local subscriber productions, splices
  only the parser-retained subscriber span across repeated length changes, and preserves all other
  wire bytes. All sixteen shared URI vectors and the focused checks reported in S-44 are green;
  story completion and the full workspace gate remain coordinator work.
- 2026-08-05: Adversarial review caught one over-restriction in the local-number production: RFC
  3966 permits `*` or `#` as the required non-separator symbol, not only a hexadecimal digit. The
  revised UR-T-5 regression uses dial-symbol-only `*#` and the validator now accepts the complete
  production.

- 2026-08-05: Integration's single full-gate invocation passed repository checks, workspace clippy
  and the complete workspace test suite, then stopped itself before `examples` because the cold
  build exhausted the disk floor. It was an infrastructure non-result and was not rerun.

- 2026-08-05: the protected beta.7 workflow completed the full repository gate at the immutable
  release tag. Every acceptance item is now satisfied and the story closes with that exact evidence.

## Notes

- Considered for the kernel: yes. RFC 3966 subscriber syntax, delimiter ownership and trustworthy
  raw spans belong to `sipx-sip`; deciding which fields or numbers routing policy rewrites remains
  application policy.
- The existing `Uri::equivalent` implementation already contained the TEL split privately. S-48
  exposes and mutates at that parse boundary rather than introducing a second algorithm.
