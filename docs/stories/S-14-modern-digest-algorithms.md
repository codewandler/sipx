---
id: S-14
title: Add the modern digest algorithms
pillar: Signalling
status: done
priority: 5
design:
epic: conformance
areas: [sipx-ua]
note: track: auth · RFC 8760 · sipx-ua only, isolated
---

# Add the modern digest algorithms

## Goal
SHA-512-256, and the rules for a server that offers several algorithms at once.

## Acceptance
- [x] SHA-512-256 and SHA-512-256-sess alongside the existing MD5 and SHA-256.
- [x] A challenge offering several algorithms is answered with the strongest sipx supports, not
      the first one listed.
- [x] Checked against published test vectors rather than against sipx's own arithmetic, as the
      existing digest implementation is.
- [x] Failing-first test: `the_strongest_offered_algorithm_is_chosen`.

## Progress
- Done. `SHA-512-256` and `SHA-512-256-sess` alongside MD5 and SHA-256.
- **The selection rule is a deliberate departure from a SHOULD, and is documented as one.**
  RFC 8760 §2.4 says the UAC "SHOULD use the topmost header field that it supports *unless a
  local policy dictates otherwise*". sipx ranks by strength instead, because §3 of the same
  document names what the ordering enables: a challenge is not integrity-protected, so an
  on-path attacker can reorder the header fields to put MD5 on top, and a client that honours
  the order complies. `topmost_supported` is exported for a deployment where the server's
  ordering carries information this client does not have. Ties go to the earlier challenge, so
  the server still decides where strength does not.
- **Both digest vectors are now published ones.** The previous SHA-256 test asserted a value
  "computed independently" — computed, that is, by the same reasoning that wrote the code. A
  digest agreeing with itself proves nothing. It is now RFC 7616 §3.9.1 verbatim, which also
  pulled in errata 4495: the password is "Circle of Life" with a lowercase "of".
- The SHA-512-256 vector needed care. **RFC 7616 §3.9.2 as printed does not reproduce**, and its
  erratum (4897) is still "Reported" rather than "Verified", so neither source is authoritative
  alone. Two things make it usable: the erratum's `response` was derived independently by its
  reporter and matches what sipx computes, and the erratum's *userhash* — a separate digest over
  different input — reproduces too. Two independent values agreeing is a far stronger signal
  than either alone, and both are asserted.
- The username in that vector carries U+00E4 and U+00F8 deliberately: `A1` is built from raw
  UTF-8 octets, and an implementation that mangled the encoding would still pass an ASCII-only
  vector.
- Mutation-tested: computing SHA-512/256 as a truncated SHA-512, ranking it below MD5, and
  reverting the tie-break to `max_by_key` each fail exactly the test that names them.
