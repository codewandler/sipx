---
id: X-12
title: Write the user-facing guides
pillar: Build
status: ready
priority: 2
design:
epic: docs-site
areas: [docs]
note: track: docs · nothing else touches docs/
---

# Write the user-facing guides

## Goal
The pages someone actually needs: what sipx is for, what it can and cannot do yet, and how to
use it for the two or three things people will want first.

## Acceptance
- [x] A "does this fit" page: what sipx is, what it is not, and the honest state of it —
      including that media is not encrypted yet.
- [x] A page per thing someone would arrive wanting: place a call, answer one, register against
      a PBX, use it as a library.
- [x] Every code sample compiles, checked in CI. A sample that has rotted is worse than none,
      because it is trusted.
- [x] The compliance table is linked from wherever a capability is claimed, so a claim can be
      checked rather than believed.

## Progress
- Done. Five pages: "Does sipx fit?", then one each for placing a call, answering one,
  registering, and using the crates as a library.
- **The samples are compiled files, not quoted prose.** Each guide pulls in a real
  `crates/*/examples/*.rs` with mdBook's `{{#include}}`, and CI builds them. A sample that no
  longer compiles is worse than no sample, because it is read as working code — so the way to
  make that claim true was to stop writing code into markdown.
- `build-docs.sh` also checks every include resolves, which the link checker cannot see: a
  missing include renders as an error message inside a code block rather than as a broken link.
- Writing the "does this fit" page immediately caught a claim that had just gone stale: the
  README and the landing page both said media was not encrypted, which `M-14` made false an hour
  earlier. Both corrected — and the replacement says what the encryption does *not* cover
  (SDES puts the key in the SDP body, one transform, no rekeying) rather than declaring victory.
