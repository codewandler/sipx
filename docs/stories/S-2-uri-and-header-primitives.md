---
id: S-2
title: Implement SIP URIs, header names and header parameters
pillar: Signalling
status: done
priority:
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
- [x] `Uri` parses and re-serializes per RFC 3261 §19.1, including user, password, host,
      port, URI parameters, headers, and correct escaping of reserved characters (§19.1.2).
- [x] Comparison follows the RFC's equivalence rules (§19.1.4) — not string equality: known
      parameters compare case-insensitively where specified, `transport`/`user`/`ttl`/`method`
      are significant, unknown parameters must match only if present in both.
- [x] `HeaderName` round-trips compact forms (§7.3.3, §20) and compares case-insensitively
      while preserving the original spelling for verbatim output.
- [x] Parameter lists preserve order and duplicate keys, since forwarding must not reorder
      them.
- [x] Failing-first test: `uri_equivalence_rfc3261_19_1_4` covering the RFC's own worked
      examples, which fail under naive string comparison.
- [x] Property test: parse → serialize → parse is a fixed point for generated URIs.

## Progress
- Done. `Uri`, `Host`, `Scheme`, `HeaderName`, `Params` in `crates/sipx-sip/`, 31 tests.
- The equivalence relation is **not transitive** — the RFC says so and gives the example. So
  `Uri` deliberately does not implement `PartialEq` as equivalence; a non-transitive `PartialEq`
  breaks `HashMap`, sorting and every reader's assumption. Protocol logic calls
  `Uri::equivalent`. There is a test asserting the non-transitivity, so nobody "fixes" it.
- Parsing order matters more than expected: the user part may contain `?`, `;` and `/`
  (RFC 4475 3.1.1.2, 3.1.1.9), so userinfo must be split off *before* scanning for parameters
  or headers. An unescaped `@` is unambiguous because it appears in no other character set.
- `HeaderName` hashing lowercases the canonical form so hashing agrees with case-insensitive
  equality; a test keys a `HashSet` by `Via` and looks it up as `v`.
- Percent escapes are never decoded while parsing. `decoded_user()` is explicit and returns
  bytes, because RFC 4475 3.1.1.4 has a user part of `null-%00-null`.

## Notes
- Depends on `S-1` for the representation decisions (borrowed vs. owned).
