---
id: S-47
title: Expose lossless nested URI editing
pillar: Signalling
status: in-progress
priority: 3
design: docs/specs/lossless-message-editing.md
epic:
areas: [sipx-sip, routing]
predicate:
announcement:
note: parser-owned request-line and address-list surgery for forwarding consumers
---

# Expose lossless nested URI editing

## Goal

Let a forwarding consumer replace Request-URI and address-header URIs, or remove a selected
address-list value, without rebuilding or byte-searching the enclosing syntax.

## Acceptance

- [x] Parsed Request-URI replacement changes only the parser-owned URI span; constructed requests
      remain deterministic and repeated replacement stays correct.
- [x] Address edits cover every existing typed address header plus asserted/preferred identity,
      use stable flattened wire-order indices across repeated rows, and never locate by byte search.
- [x] URI replacement preserves all enclosing bytes and selected-value removal preserves every
      surviving row and value, including supported folds and comma whitespace.
- [x] Unsupported headers, malformed addresses, out-of-range indices and invalid replacement URIs
      are typed, atomic failures with no panic path.
- [x] The normative specification precedes the implementation and the byte-vector tests include
      ambiguous display text, folding, comma lists, repeated rows and removals.
- [x] Focused formatting, clippy, tests and documentation checks are green.

## Progress

- 2026-08-05: filed from the routing consumer review after byte-search was shown ambiguous when a
  display name equals the URI. The protocol-generic contract and fourteen byte vectors were written
  before implementation.
- 2026-08-05: the integration target failed first on the absent Header/Headers surgery operations.
  The address parser now returns the URI range from the same pass that constructs the Address;
  request lines retain their Request-URI range, and folded fields project grammar spans back to raw
  bytes through a source map. All 10 integration tests and 241 sipx-sip unit tests pass, package
  clippy is clean with warnings denied, and formatting, RFC, link, maturity and website-sync checks
  are green. The repository-wide gate remains for integration.
- 2026-08-05: review added a candidate-reparse guard before header assignment. Bare address
  replacements that would turn URI semicolons, query delimiters or list commas into enclosing
  syntax now fail atomically, while the same standalone-valid URIs replace exactly in name-address
  form. Additional vectors prove a malformed later row blocks collection mutation and that removing
  a final value retains trailing whitespace and folding.

- 2026-08-05: Integration's single full-gate invocation passed repository checks, workspace clippy
  and the complete workspace test suite, then stopped itself before `examples` because the cold
  build exhausted the disk floor. It was an infrastructure non-result and was not rerun.

## Notes

- Considered for the kernel: yes. The parser owns the only trustworthy nested syntax spans; routing
  and identity policy remain consumers of these operations.
