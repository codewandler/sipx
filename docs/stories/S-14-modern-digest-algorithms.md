---
id: S-14
title: Add the modern digest algorithms
pillar: Signalling
status: ready
priority: 9
design:
epic: conformance
areas: [sipx-ua]
note: RFC 8760; small and self-contained
---

# Add the modern digest algorithms

## Goal
SHA-512-256, and the rules for a server that offers several algorithms at once.

## Acceptance
- [ ] SHA-512-256 and SHA-512-256-sess alongside the existing MD5 and SHA-256.
- [ ] A challenge offering several algorithms is answered with the strongest sipx supports, not
      the first one listed.
- [ ] Checked against published test vectors rather than against sipx's own arithmetic, as the
      existing digest implementation is.
- [ ] Failing-first test: `the_strongest_offered_algorithm_is_chosen`.

## Progress
- Not started. `compliance.md` has RFC 7616 implemented and RFC 8760 not started.
