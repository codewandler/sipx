---
id: X-66
title: Measure coverage and publish the number
pillar: Build
status: ready
priority: 8
design:
epic: conformance
areas: [scripts, ci]
predicate:
announcement:
note: 1756 test attributes and no measurement of what they reach · a number that is generated, never asserted · follow-up
---

# Measure coverage and publish the number

## Goal

Generate a coverage figure from the suite rather than having none, so the question "what does this
suite not reach" has an answer that does not depend on somebody reading 79 test files.

## Acceptance

- [ ] A CI job measures workspace line and branch coverage with `cargo llvm-cov` and publishes the
      result as a build artifact.
- [ ] The figure is **generated into the docs, never transcribed** — the same rule
      `docs/compliance.md` and `docs/maturity.md` already follow. A hand-written percentage anywhere
      in the tree fails this story.
- [ ] The job is registered in `scripts/gate.py`, either as a step or in `NOT_RUN_LOCALLY` with a
      stated reason, so `gate.py --check` stays green and the gate cannot silently omit it.
- [ ] The published figure states what it excludes (examples, fuzz targets, generated code) on the
      same surface that states the number.
- [ ] **No coverage threshold gates the build in this story.** A ratchet is a separate decision; see
      Notes.
- [ ] `./scripts/gate.py` green.

## Progress
- (not started)

## Notes
- Found by the 2026-08-04 capability review: there is no `cargo llvm-cov` or tarpaulin step anywhere
  in the gate or CI, and `docs/maturity.md` already names the deeper limit honestly — "nothing here
  measures whether the tests are good, only that they pass". Coverage does not fix that; it bounds it.
- Deliberately no threshold. A coverage gate rewards tests written to touch lines, which is the
  `X-36` failure shape in a new place: it looks like coverage and is not. Measure first, decide about
  a ratchet later with the number in hand.
- Follow-up, not beta-1: it changes nothing a user can observe and blocks no announcement predicate.
