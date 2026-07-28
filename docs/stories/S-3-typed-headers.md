---
id: S-3
title: Implement typed headers with verbatim passthrough
pillar: Signalling
status: done
priority:
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip]
note:
---

# Implement typed headers with verbatim passthrough

## Goal
Give the headers that carry protocol semantics real types, while guaranteeing that headers
sipx does not understand survive a forward byte for byte.

## Acceptance
- [x] Typed: `Via` (with `branch`, `received`, `rport`, `maddr`, `ttl`), `From`, `To`,
      `Call-ID`, `CSeq`, `Contact`, `Route`, `Record-Route`, `Max-Forwards`, `Expires`,
      `Content-Type`, `Content-Length`, `Date`, `Allow`, `Supported`, `Require`,
      `Proxy-Require`, `Unsupported`.
- [ ] `Authorization`, `WWW-Authenticate`, `Proxy-Authorization`, `Proxy-Authenticate` —
      **deferred to the user-agent epic** (`sip-ua`). Nothing in the core can act on a
      challenge, and a type nobody uses is a type nobody has tested. Carried as a story there.
- [x] Multi-value headers handle both repeated header lines and comma-separated values on one
      line (RFC 3261 §7.3.1), and know which headers may **not** be combined that way.
- [x] Unknown headers are retained with their original bytes, order and spelling; a
      parse-then-serialize of any corpus message is byte-identical unless a header was
      deliberately modified.
- [x] Failing-first test: `unknown_headers_survive_roundtrip_byte_exact`.
- [x] Header list order is preserved end to end, including `Via` stacking order, which is
      load-bearing for routing.

## Progress
- Done. `crates/sipx-sip/src/headers/` plus `validate.rs`. The whole RFC 4475 classification
  is now green across all four layers: 27 parse-ok, 9 structural rejects, 7 value-level
  rejects in the *named* header, 6 semantic rejects from validation.
- Authentication headers are deliberately deferred to the user-agent epic: nothing in the core
  can act on a challenge, and a type nobody uses is a type nobody has tested.
- The `addr-spec` trap is worth remembering: without angle brackets a semicolon starts a
  *header* parameter, so `sip:a@b;tag=1` is a URI plus one header parameter, while
  `<sip:a@b;tag=1>` is one URI with a URI parameter and no header parameters. Two characters,
  entirely different meanings; there is a test pinning both.
- Empty parameter segments are now rejected in URIs as well as in headers. The ABNF's `pname`
  is `1*paramchar` in both places, and the inconsistency would have been a wart.
- `Contact: *` needed its own type. It is the one place a header that otherwise holds
  addresses holds an asterisk, and a parser expecting an address there rejects a legal
  deregistration.
- Validation returns a list of findings rather than a first error, and marks a missing
  `Max-Forwards` repairable, because RFC 3261 §16.6 lets a proxy add one instead of rejecting.

## Notes
- The verbatim guarantee is what makes a proxy possible later without reparsing.
