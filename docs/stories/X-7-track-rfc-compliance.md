---
id: X-7
title: Track RFC compliance, measured rather than asserted
pillar: Build
status: done
priority: 1
design:
epic: conformance
areas: [docs]
note: seeds the conformance epic
---

# Track RFC compliance, measured rather than asserted

## Goal
A list of the RFCs sipx tracks, from the wire upward, saying honestly how far each one is
implemented — and a check that stops the list drifting away from the code.

## Acceptance
- [x] A machine-readable registry is the source; the published table is generated from it.
- [x] Four states are distinguishable: implemented, partial, syntax-only, not started. Syntax
      only is its own state, because "we support RFC 3262" and "we reject it" are both false
      of a stack that parses `RAck` and does nothing with it.
- [x] Every entry claiming implementation cites the code or tests that back it.
- [x] A check verifies the claims: a named header must be known to the parser, a cited file must
      exist, and the generated table must be current.
- [x] Roles are recorded per RFC, so proxy behaviour sipx does not implement is not implied by
      a UA implementation of the same document.
- [x] Linked from the README.

## Progress
- Done. `docs/rfc/registry.toml` is the source, `scripts/rfc-report.py` generates
  `docs/compliance.md` and verifies it, and CI runs the check.
- 61 RFCs: 22 implemented, 7 partial, 10 syntax-only, 21 not started, 1 superseded.
- The check earned its place immediately by rejecting the first draft — an entry cited
  `crates/sipx-sip/tests/rfc4475.rs`, which does not exist. A hand-written table would have
  shipped that.
- What it deliberately does not do is verify behaviour. No script can read a transaction
  machine and decide whether Timer A is right; the tests do that, and each entry points at them.
  What it stops is the failure mode a compliance table actually has — drifting from the code
  until it reads as marketing.

## Notes
- The roadmap that goes with it: `docs/rfc-roadmap.md`.
