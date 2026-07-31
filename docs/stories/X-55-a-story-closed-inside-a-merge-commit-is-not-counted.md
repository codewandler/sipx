---
id: X-55
title: Count a story closed inside a merge commit, or refuse to be closed inside one
pillar: Build
status: done
priority: 3
epic: conformance
areas: [scripts]
note: found integrating M-34 — its `status: done` landed in the merge commit, `git log -p` shows no diff for merges, so the closing was invisible to the history walk and the journal came out one ahead of the snapshot
---

# Count a story closed inside a merge commit, or refuse to be closed inside one

## Goal
Stop `maturity.py` silently under-counting a story fact, and stop the report needing a hand repair
when one lands in a merge commit.

## Acceptance
- [x] **Reproduce it.** `history_story_fact_days` walks
      `git log --date=short --format=... -p --unified=0 -- docs/stories`. `git log -p` emits **no
      diff for a merge commit** unless asked, so a `status: done` line that first appears in a merge
      is never seen. Build the case in `test-maturity.py`'s fixture repository: branch, close a story
      on the branch, merge with `--no-ff`, and assert the closing is counted. It will not be.
- [x] **The same hole exists for `filed`.** `--diff-filter=A --name-only` is a separate `git log`
      invocation with the same default. A story file that first appears in a merge commit is not
      counted as filed either. Cover both.
- [x] **Decide which way to close it, and say why.** Either count merge diffs — `--diff-merges=1`
      or `-m --first-parent` — which changes the counts of *every* past merge in this repository and
      must be checked against the committed report before it is adopted; or detect the case and fail
      with a diagnostic naming it, so the fact is never silently lost. The second is cheaper and in
      the spirit of `X-49`; the first is more correct. Do not do both.
- [x] **The diagnostic, if that is the route, names the cause.** *(Not the route: merge diffs are
      counted, so no diagnostic for this case exists to name it. Both existing journal diagnostics
      gained the repair command regardless — see the item below and `RESEED_ADVICE`.)* `X-49`'s lesson was that
      "the journal records 141 and the snapshot has 140" points the reader at the one file that is
      not wrong. Whatever this reports must name the merge commit and the story.
- [x] **The report must not need hand repair.** Recovering from this took editing the generated
      journal out of `docs/maturity.md` and regenerating, because the committed journal is read as a
      floor and a hand-edited count then fails the basis hash. Whatever the fix, a maintainer who
      hits this should have a documented command, not a reverse-engineered one.

## Progress
- Filed 2026-07-30, hit while integrating `M-34`. The immediate repair was to drop the
  `maturity-event-days` line from `docs/maturity.md`, stage it, and regenerate — which rebuilds the
  journal from committed history. Today's `closed` row went from 36 to 35 as a result, and that 35
  is the honest number under the current counting rule: `M-34`'s close is not in any non-merge diff.
- Closed 2026-07-31. Merge diffs are counted, and the Notes' hypothesis above turned out to be
  **false**: `--diff-merges=first-parent` alone does not reproduce the committed rows, it takes filed
  from 182 to 224 and closed from 144 to 180, because `git log` walks every parent and a branch fact
  is counted once on the branch and again in the merge that brought it in. `--first-parent` alongside
  it is what makes a story fact an event on the mainline, counted exactly once wherever it was
  written. Verified independently at integration: filed 182 → 181 (`S-26` had been counted as filed
  twice, from two independent creations of one file) and closed 144 → 146 (`M-34` and `S-26`).
- The implementor was killed mid-run by an org monthly spend limit, so its work was rescued to
  `impl/X-55` as a commit and the **failing-first proof was established at integration instead**:
  with `scripts/maturity.py` reverted to the base and the new cases in place, `test-maturity.py`
  reports 5 failures and 7 errors, the filing case failing because the merge-commit story is absent
  from the fact list. Restored, 55 tests green. Reviewed by one context rather than two — see below.

## Notes
- **Reads with `X-49`**, which fixed the other way this counter could be wrong — CI comparing the
  journal against a history it had not fetched. Same measurement, same failure shape: a count that
  is quietly a function of how the history is read rather than of what happened.
- The convention that avoids it is already written down in the impl-coord flow: the merge commit and
  the ledger commit are separate, and the ledger commit is an ordinary one. This story is about the
  script not depending on a human remembering that.
- Worth checking whether `git log --diff-merges=first-parent` reproduces the currently committed
  day rows exactly. If it does, adopting it is nearly free and strictly better.
