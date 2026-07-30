# Design: commit-stable generated measurements

**Status:** proposed · **Pillar:** Build · **Epic:** `commit-snapshot` · **Stories:** X-39

## Why

A generated measurement that is green only in the worktree where it was written is not a gate. The
maturity report combines committed history with facts that are about to enter history, so it needs a
precise snapshot and date model. X-39 fixed the ordinary all-changes path, but the 2026-07-30 repository
review found two remaining ways for the same report to change across its own commit: selective staging
and date attribution.

## Approach

Treat report generation as a prediction of one concrete commit snapshot:

1. committed facts come from history;
2. pending facts come from the staged story snapshot that will be committed, not from unstaged or
   untracked files;
3. a pending fact receives a date key that is available before commit and remains its key after the
   commit, including across midnight and an amend that retains an older author date; and
4. tests compare the originating worktree with a clean checkout of the produced commit.

The implementation must define the date rule before changing code. If Git cannot provide a stable
pre-commit date for the chosen history field, the report format must represent pending facts without
pretending their final day is known. It must not silently substitute the wall clock.

## Alternatives considered

- Read every local change. This makes the common all-changes path convenient but reports content that
  a selective commit does not contain.
- Ignore the current day. That delays the mismatch until the day changes and hides real drift while
  the row is most active.
- Use the generator's wall clock as the history date. That differs from retained author dates and can
  cross midnight before the commit is created.

## Risks and open questions

- A staged-only model must still handle a newly staged story whose worktree copy has later edits.
- Closing facts require comparing the staged snapshot with the correct parent snapshot, not scanning
  an arbitrary working diff.
- Amend and merge commits need explicit test fixtures so the date rule is not inferred from the
  common single-parent case.

## Acceptance / done

The epic is done when X-39 is done and `maturity.py --check` gives the same answer before the commit
and in a clean checkout of it for all-changes, selective, midnight-boundary and retained-author-date
amend cases.
