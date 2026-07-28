---
id: S-3
title: Implement typed headers with verbatim passthrough
pillar: Signalling
status: backlog
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
- [ ] Typed: `Via` (with `branch`, `received`, `rport`, `maddr`, `ttl`), `From`, `To`,
      `Call-ID`, `CSeq`, `Contact`, `Route`, `Record-Route`, `Max-Forwards`, `Expires`,
      `Content-Type`, `Content-Length`, `Allow`, `Supported`, `Require`, `Authorization`,
      `WWW-Authenticate`, `Proxy-Authorization`, `Proxy-Authenticate`.
- [ ] Multi-value headers handle both repeated header lines and comma-separated values on one
      line (RFC 3261 §7.3.1), and know which headers may **not** be combined that way.
- [ ] Unknown headers are retained with their original bytes, order and spelling; a
      parse-then-serialize of any corpus message is byte-identical unless a header was
      deliberately modified.
- [ ] Failing-first test: `unknown_headers_survive_roundtrip_byte_exact`.
- [ ] Header list order is preserved end to end, including `Via` stacking order, which is
      load-bearing for routing.

## Progress
- Not started.

## Notes
- The verbatim guarantee is what makes a proxy possible later without reparsing.
