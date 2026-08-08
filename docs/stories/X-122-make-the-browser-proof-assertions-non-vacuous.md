---
id: X-122
title: Make the browser proof assertions non-vacuous
pillar: Build
status: in-progress
priority: 4
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

- [x] A test that mutates a positive fact rebinds the negatives that reference it, so the validator
      reaches the field under test rather than refusing on the SHA-256 binding.
- [x] `test_every_positive_fact_is_asserted` is proved non-vacuous: a validator stubbed to ignore
      the field it names must make it fail.
- [x] Every other proof assertion is audited the same way, and any found vacuous is either fixed or
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
- 2026-08-08: the defect is confirmed and measured rather than argued. `PROOF_ASSERTIONS` in
  `scripts/test-browser-audio-proof.py` lists all 20 assertions this suite makes about
  `validate_proof`, across the 10 tests that reach it: 17 are proved load-bearing by
  `test_every_proof_assertion_is_non_vacuous`, which re-runs each named test against a validator
  blinded to exactly the refusals it rests on, and 3 record why no stub can reach them. At the
  merge base every one of the 7 fields `test_every_positive_fact_is_asserted` names passed blind,
  and the 8th crashed the validator. Three further vacuous assertions were found elsewhere in the
  suite and fixed; two requirements in `tests/browser-audio/driver.py` were made independently
  removable so they could be measured at all.
  `test_the_audit_covers_every_assertion_about_the_validator` fails if a test reaches
  `validate_proof` without a row, so the audit cannot rot by omission. `./scripts/gate.py` is
  the coordinator's per-wave run and was not run here.
- 2026-08-08: `PEER_KEYINGS` was deliberately left alone — it needs a transform dimension in
  `tests/interop/run.sh`, which is a different file and a different claim.

## Notes

- This is the same class as a coverage number that counts its own tests: the artefact looks like
  evidence and is not. `M-72`'s `rebind_negatives()` is the mechanism to generalise.
- Related but separate: `PEER_KEYINGS` conflates keying with transform — a peer declaring `sdes` or
  `dtls` says nothing about which suite it settles on, which is how the AEAD blind spot stayed
  invisible. Fixing that means a transform dimension in `tests/interop/run.sh`.
