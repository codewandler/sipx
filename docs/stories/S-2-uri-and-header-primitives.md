---
id: S-2
title: Implement SIP URIs, header names and header parameters
pillar: Signalling
status: ready
priority: 3
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip]
note:
---

# Implement SIP URIs, header names and header parameters

## Goal
Build the primitives every other part of the core is expressed in: `sip:`/`sips:`/`tel:` URIs,
header names with their compact forms, and the parameter lists that hang off both.

## Acceptance
- [ ] `Uri` parses and re-serializes per RFC 3261 §19.1, including user, password, host,
      port, URI parameters, headers, and correct escaping of reserved characters (§19.1.2).
- [ ] Comparison follows the RFC's equivalence rules (§19.1.4) — not string equality: known
      parameters compare case-insensitively where specified, `transport`/`user`/`ttl`/`method`
      are significant, unknown parameters must match only if present in both.
- [ ] `HeaderName` round-trips compact forms (§7.3.3, §20) and compares case-insensitively
      while preserving the original spelling for verbatim output.
- [ ] Parameter lists preserve order and duplicate keys, since forwarding must not reorder
      them.
- [ ] Failing-first test: `uri_equivalence_rfc3261_19_1_4` covering the RFC's own worked
      examples, which fail under naive string comparison.
- [ ] Property test: parse → serialize → parse is a fixed point for generated URIs.

## Progress
- Not started.

## Notes
- Depends on `S-1` for the representation decisions (borrowed vs. owned).
