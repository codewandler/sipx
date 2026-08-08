---
id: X-122
title: Make the browser proof assertions non-vacuous
pillar: Build
status: ready
priority: 11
design:
epic: test-surfaces
areas: [scripts, tests]
predicate:
announcement:
note: every negative carries the hash of its positive, so a mutation is refused on the binding alone and a test can pass without checking the field it names
---

# Make the browser proof assertions non-vacuous

## Goal

Make each browser-audio proof assertion actually exercise the field it names, instead of passing
because the evidence binding refused the mutation before the field was ever read.

## Acceptance

- [ ] A test that mutates a positive fact rebinds the negatives that reference it, so the validator
      reaches the field under test rather than refusing on the SHA-256 binding.
- [ ] `test_every_positive_fact_is_asserted` is proved non-vacuous: a validator stubbed to ignore
      the field it names must make it fail.
- [ ] Every other proof assertion is audited the same way, and any found vacuous is either fixed or
      removed — a test that cannot fail is worse than no test, because it reads as coverage.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `M-72`'s adjacent findings, which it called the more interesting result of
  that story. Each negative record carries the SHA-256 of the positive it was recorded against, so
  **any** mutation to a role's evidence makes `validate_proof` refuse on the binding alone —
  whether or not the field under test is checked. `M-72`'s first draft of all three of its new tests
  passed at the merge base for exactly that reason. It added `rebind_negatives()` for its own tests;
  the pre-existing `test_every_positive_fact_is_asserted` still mutates `browser-offerer` evidence
  without rebinding, so it would pass against a validator that ignored every field it lists.

## Notes

- This is the same class as a coverage number that counts its own tests: the artefact looks like
  evidence and is not. `M-72`'s `rebind_negatives()` is the mechanism to generalise.
- Related but separate: `PEER_KEYINGS` conflates keying with transform — a peer declaring `sdes` or
  `dtls` says nothing about which suite it settles on, which is how the AEAD blind spot stayed
  invisible. Fixing that means a transform dimension in `tests/interop/run.sh`.
