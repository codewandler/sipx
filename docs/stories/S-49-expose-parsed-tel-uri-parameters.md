---
id: S-49
title: Expose parsed TEL URI parameters
pillar: Signalling
status: ready
priority: 2
design: docs/specs/uri-rewriting.md
epic: sip-core
areas: [sipx-sip, uri]
predicate:
announcement:
note: requested by sipx-clstr CX-18 — routing consumers must not split the raw RFC 3966 parameter tail
---

# Expose parsed TEL URI parameters

## Goal

Let protocol consumers inspect RFC 3966 TEL parameters without splitting the raw parameter tail,
reimplementing its delimiter rules or performing case-sensitive parameter-name comparisons.

## Acceptance

- [ ] The URI-rewriting contract cites RFC 3966 §§3–4 and defines an allocation-free, typed
      parameter iterator before code. Each item borrows its exact name and optional value bytes,
      and parameter-name comparison is parser-owned and ASCII case-insensitive.
- [ ] The iterator distinguishes no parameter tail from one or more parameters. It preserves input
      order and duplicate occurrences rather than silently coalescing them, so consumers can apply
      policy without losing evidence.
- [ ] Generic parameter names and values are checked against RFC 3966's `pname` and `pvalue`
      productions. Percent-escaped value bytes remain exact and cannot create a structural `;` or
      `=` delimiter.
- [ ] Empty segments, empty names, empty values and illegal name/value bytes yield a typed error at
      the offending byte offset. After that error the iterator is fused; malformed input is never
      reported as a trustworthy partial parameter set.
- [ ] Failing-first public integration vectors cover an absent tail, `phone-context`, `ext` with no
      context, reordered parameters, case-insensitive names, escaped bytes, duplicates and
      malformed tails.
- [ ] Existing raw `TelUriParts::parameters()` and lossless subscriber mutation remain byte-exact,
      and focused formatting, clippy, unit, feature-off and documentation checks pass.

## Progress

- 2026-08-05: Filed from downstream CX-18 after the beta.7 consumer proved the remaining API gap.
  `TelUriParts::parameters()` deliberately returns an uninterpreted byte tail, while the only TEL
  parameter splitter remains private to URI equivalence. The public `s49_*` vectors fail to compile
  because `TelUriParts::parsed_parameters` does not exist; the failure occurs before any downstream
  delimiter logic can be introduced.

## Notes

- Requested by downstream
  [sipx-clstr CX-18](https://github.com/codewandler/sipx-clstr/blob/main/docs/stories/CX-18-file-parser-owned-tel-parameter-lookup.md)
  through its [upstream ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md).
  The kernel owns RFC 3966 syntax and parameter-name comparison. The downstream platform retains
  the policy decision that a local number requires `phone-context` before it may be rewritten.
- This surface parses generic syntax only. It does not choose a `phone-context`, normalise a value,
  reject duplicate extension parameters as policy, or decide whether a context is suitable.
