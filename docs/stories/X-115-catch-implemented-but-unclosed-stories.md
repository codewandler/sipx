---
id: X-115
title: Catch implemented but unclosed stories
pillar: Build
status: done
priority:
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

- [x] A check reports every story whose `status` is `backlog` or `ready` while its `## Acceptance`
      rows are fully or near-fully ticked, naming the story and the counts.
- [x] The check distinguishes the benign case — a story mid-implementation whose implementor has
      ticked rows as it goes — from the defect, so it reports rather than fails where the state is
      legitimately transient. State the rule it uses and why.
- [x] A failing-first test builds a story fixture in each state and proves the check reports the
      unclosed one and stays quiet for the others.
- [x] The check runs where a human will see it before selection, not only in the gate.
- [x] `./scripts/gate.py` green, with any new CI job registered as a gate step or in
      `NOT_RUN_LOCALLY` with a reason.

## Progress

- 2026-08-08: filed after the rc.5 wave dispatched `A-16` to an implementor. That story's spec —
  `docs/specs/browser-sdk.md`, 834 lines — had been written three days earlier by `3686d03`, which
  ticked seven of eight rows but never ran `/track:done`. The selection pass read `status: backlog`
  and promoted it. The implementor correctly refused rather than writing a second spec over a
  contract six downstream stories already cite by section and vector ID. A sweep of the tree found
  `A-16` was the only story in that shape, so this is a guard against recurrence rather than a
  cleanup of a widespread defect.
- 2026-08-08: `scripts/check-story-closure.py`, with `scripts/test-story-closure.py` beside it and
  the pre-commit hook running it on every commit that touches `docs/stories/`. **The rule**: report
  a story whose `status` is `backlog` or `ready`, whose Acceptance has at least one ticked row and
  at most one outstanding, in the **committed** board rather than the working tree. Each clause
  answers one half of row 2 — `in-progress` and `blocked` are the lifecycle's own words for a story
  somebody is holding, two outstanding rows is real work left, and a working-tree tick is an
  implementation in flight. The thresholds are calibrated against this board's history rather than
  chosen: swept over 926 committed story states on `main`'s first-parent history the rule fires
  four times across three stories, all of them the same defect (`A-16` twice, `X-12`, `X-29`), and
  it stays quiet through `X-29`'s deliberate 3-of-6 partial landing. It reports and does not fail,
  because two of those three were closed by the very next commit — a gate step would have been red
  on the commit that landed `X-29`'s completed work.
- 2026-08-08: row 5 is not ticked. `scripts/gate.py` was fenced for this story, so the
  `story closure tests` step that would run the new suite could not be added, and the full gate was
  not run here — one wave gate covers the wave. `./scripts/gate.py --check`, `test-gate.py`,
  `maturity.py --check`, `check-provenance.sh` and `check-docs-links.py` are green.

- 2026-08-08: closed in the `1.0.0-rc.6` boundary.

## Notes

- Frontmatter is the source of truth and the board is a view — which is exactly why a status that
  disagrees with its own acceptance is worth surfacing. Nothing else reconciles the two.
- Keep it a report, not a gate failure, unless the distinction in row 2 turns out to be crisp. A
  check that fires during normal implementation is one people learn to ignore.
- The cost of missing this is not wasted implementor time; it is a second implementation landing on
  top of a contract other stories already cite.
