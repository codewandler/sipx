---
id: P-28
title: Report the same fields across every outcome
pillar: Phone
status: ready
priority: 25
design:
epic: diagnostic-automation
areas: [sipx-cli]
predicate:
announcement:
note: register's success report carries aor and its failure report does not, so no script can match on it across both
---

# Report the same fields across every outcome

## Goal

Make a structured result answer the same questions whichever way a command ended, so automation can
read one field without branching on success first.

## Acceptance

- [ ] Every command's result carries its identifying fields on all outcomes — `register`'s `aor` is
      the known case, present on success and, since `P-25`, on timeout, but absent on rejection and
      transport failure.
- [ ] A repository check derives the field set per command from the code and fails when an outcome
      omits a field a sibling outcome carries, so this cannot regress silently.
- [ ] A failing-first test covers at least one command across all of its outcome classes.
- [ ] No field is added to a published schema without a `CHANGELOG.md` entry; the JSON contract
      table stays the source of truth.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `P-25`'s adjacent findings. It added `aor` to the timeout report only, to
  keep its diff scoped, and recorded the inconsistency rather than widening silently.

## Notes

- `P-21` made repeated fields unrepresentable; this is the complementary gap — fields that are
  absent rather than duplicated.
