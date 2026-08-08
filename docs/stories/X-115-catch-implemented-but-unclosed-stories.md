---
id: X-115
title: Catch implemented but unclosed stories
pillar: Build
status: ready
priority: 21
design:
epic: conformance
areas: [scripts, docs]
predicate:
announcement:
note: A-16 was fully delivered, left at status backlog, then re-selected into a wave and dispatched to an implementor
---

# Catch implemented but unclosed stories

## Goal

Make a story whose acceptance is satisfied but whose `status` was never moved visible, so wave
selection cannot re-dispatch finished work.

## Acceptance

- [ ] A check reports every story whose `status` is `backlog` or `ready` while its `## Acceptance`
      rows are fully or near-fully ticked, naming the story and the counts.
- [ ] The check distinguishes the benign case — a story mid-implementation whose implementor has
      ticked rows as it goes — from the defect, so it reports rather than fails where the state is
      legitimately transient. State the rule it uses and why.
- [ ] A failing-first test builds a story fixture in each state and proves the check reports the
      unclosed one and stays quiet for the others.
- [ ] The check runs where a human will see it before selection, not only in the gate.
- [ ] `./scripts/gate.py` green, with any new CI job registered as a gate step or in
      `NOT_RUN_LOCALLY` with a reason.

## Progress

- 2026-08-08: filed after the rc.5 wave dispatched `A-16` to an implementor. That story's spec —
  `docs/specs/browser-sdk.md`, 834 lines — had been written three days earlier by `3686d03`, which
  ticked seven of eight rows but never ran `/track:done`. The selection pass read `status: backlog`
  and promoted it. The implementor correctly refused rather than writing a second spec over a
  contract six downstream stories already cite by section and vector ID. A sweep of the tree found
  `A-16` was the only story in that shape, so this is a guard against recurrence rather than a
  cleanup of a widespread defect.

## Notes

- Frontmatter is the source of truth and the board is a view — which is exactly why a status that
  disagrees with its own acceptance is worth surfacing. Nothing else reconciles the two.
- Keep it a report, not a gate failure, unless the distinction in row 2 turns out to be crisp. A
  check that fires during normal implementation is one people learn to ignore.
- The cost of missing this is not wasted implementor time; it is a second implementation landing on
  top of a contract other stories already cite.
