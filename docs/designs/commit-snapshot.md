# Design: commit-stable generated measurements

**Status:** proposed · **Pillar:** Build · **Epic:** `commit-snapshot` · **Stories:** X-39

## Why

A generated measurement that is green only in the worktree where it was written is not a gate. The
maturity report combines committed history with facts that are about to enter history, so it needs a
precise snapshot and date model. X-39 fixed the ordinary all-changes path, but the 2026-07-30 repository
review found two remaining ways for the same report to change across its own commit: selective staging
and date attribution.

## Approach

Treat report generation as a prediction of one concrete commit snapshot. There are two deterministic
modes, selected solely by whether the index already differs from `HEAD` under `docs/stories/`:

- **Ordinary all-changes mode:** with no staged story change, story aggregates and facts come from the
  complete worktree, including valid untracked stories. This preserves the established edit →
  generate/check → `git add -A` → commit workflow.
- **Selective mode:** with any staged story change — addition, edit, rename or deletion — story
  aggregates come from the index and facts come from the `HEAD`-to-index diff. Unstaged, untracked and
  post-staging worktree edits do not participate. The sequence is stage the selected story changes →
  generate → stage the report → commit.

There is no guess based on whether the report itself is staged: selecting a story snapshot opts into
selective mode. A selective commit containing only the report while excluding every local story
change is intentionally unsupported, because it provides no story-snapshot signal and cannot carry a
report of those excluded facts. If the report is staged, no story change is staged and the worktree
contains a real story change, generation and checking stop with an explicit mixed-state error.

Git does not have an "actual date of the next commit" before that commit exists. In particular, the
generator cannot know that a later command will cross midnight or use `--amend`, whose replacement
commit retains the old author date. The date model therefore does not claim that unknowable value:

1. The generated region carries a machine-readable event-date journal containing filed and closed
   counts per day. It is generated data, not another hand-maintained source.
2. Existing history that predates the journal is seeded once from commit author dates, preserving
   the table's historical meaning.
3. A newly staged fact is assigned the local calendar day on which the report first observes it. The
   journal travels in the same staged snapshot, so that day remains the fact's committed day after
   midnight and after an amendment with a retained author date. `SOURCE_DATE_EPOCH`, when present,
   supplies this day in UTC so the boundary is reproducible; otherwise the local calendar is used.
   Once the generated report is staged, later generator invocations read its journal from the index,
   so they cannot re-date the same pending fact.
4. A clean checkout reads the journal from `HEAD`. If history contains more facts than that journal,
   the generator adds the newest unrecorded history facts using their author dates. This is the
   forgotten-regeneration path: `--check` still fails because the computed journal and table differ
   from the committed report.
5. The journal has exactly `basis`, `filed` and `closed`; the latter two map ISO `YYYY-MM-DD` dates to
   positive integer counts, while `basis` binds those dates to the filed and closed fact identities
   in the selected snapshot. A fact identity is its kind and story path; repeated facts remain a
   multiset. Invalid structure, a basis mismatch and totals that cannot reconcile with committed plus
   pending facts are errors, not inputs to repair heuristically. A history rewrite that removed facts
   thus requires an explicit report regeneration from a valid base rather than silently retaining or
   redistributing events that no longer exist.

The journal records day counts plus a semantic-fact basis rather than commit identities. That is
enough for the invariant: in the normal path the committed history and journal totals are equal; a
normal commit or an amend moves a staged fact into history without changing its identity, total or
recorded day. Tests compare the originating dirty worktree with a clean checkout of the produced
commit.

"Story" has one content definition on both sides: a non-reserved Markdown file with frontmatter
carrying an `id`. Historical additions and closing lines inspect the file content in that commit,
rather than treating every `.md` filename as a story. A staged scratch note therefore remains a
non-story after commit instead of changing the count in clean CI.

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
- An amend that removes an already-recorded fact makes history shorter than the journal and therefore
  produces an inconsistent-journal error. The journal must be rebuilt deliberately; guessing whether
  the next command is an amend would make the ordinary-commit snapshot wrong instead.

## Acceptance / done

The epic is done when X-39 is done and `maturity.py --check` gives the same answer before the commit
and in a clean checkout of it for all-changes, selective, midnight-boundary and retained-author-date
amend cases.
