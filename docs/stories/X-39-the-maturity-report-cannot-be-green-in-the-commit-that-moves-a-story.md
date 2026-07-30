---
id: X-39
title: Stop the maturity report from making the gate red for something that is not a defect
pillar: Build
status: ready
priority: 3
design: docs/designs/commit-snapshot.md
epic: commit-snapshot
areas: [docs]
predicate: 3
note: alpha predicate 3 — `maturity.py --check` cannot pass in the commit that files or closes a story, because Filed/Closed for today come from git history and the fact does not exist until the commit does; regenerated twice on 2026-07-30 for no defect either time
---

# Stop the maturity report from making the gate red for something that is not a defect

## Goal
Make `./scripts/maturity.py --check` fail only when the report is wrong about something that matters.
Today it fails in every commit that files or closes a story, which is most commits, and the failure
says nothing about the code.

## Acceptance
- [x] **A commit that files or closes a story can be gate-green without a second commit.** Reproduce
      first: on a clean tree, add a story file, run `./scripts/maturity.py`, commit both, then run
      `./scripts/maturity.py --check` — it exits 1, and the diff is one number in the *Discovery versus
      closure* table's current-day row. The cause is that `Filed` comes from
      `git log --diff-filter=A -- docs/stories` and `Closed` from scanning committed diffs for a
      `status: done` line (`scripts/maturity.py:160,175`), so the count the report must contain is
      created *by the commit that contains the report*. No ordering of `maturity.py` and `git commit`
      can satisfy it, which is why it was regenerated twice on 2026-07-30 — `cffb6ed` and `4014b95` —
      with no defect either time.
- [x] **The fix does not stop the table measuring what it measures.** Discovery-versus-closure is the
      one place the report says something the board cannot, and `X-32` added it deliberately: burn-down
      is not a maturity signal while discovery outpaces closure. Deleting the current-day row, or the
      table, is not the answer — the crossover date is the number to watch. State which of these it is
      and why: the check tolerates the in-flight day; the report renders that day as provisional; or
      the day rows come from somewhere that does not move under the commit that writes them.
- [x] **`--check` still catches the drift it was built to catch.** It has earned itself twice: on its
      first run (`X-32`, recorded in the changelog) and on 2026-07-30, when `main` was found red for
      real because closing `S-25` moved the board aggregates and nothing re-ran the script. A fix that
      makes the check tolerant enough to miss *that* is worse than the flapping, because the
      report is linked as a measurement.
- [x] **Alpha predicate 3 is honest again.** The predicate is "a red gate means a defect", and
      `maturity.md` reports it as met while the gate's own `maturity` step is red for no defect. The
      predicate is documented as load-bearing for every other predicate — all of them are asserted by
      the gate — so a gate that cries wolf here weakens the other six. Note the recursion and handle
      it: this story closing will itself move the table.
- [x] Failing-first test: extend `test-maturity` with a case that files a story, regenerates, commits
      and asserts `--check` is green. Name it. It must fail before the fix.
- [ ] **A selective commit reports the snapshot that will enter history, not the rest of the local
      tree.** Stage one new or closing story and the generated report while leaving another story
      unstaged or untracked; the local check and a clean checkout of the resulting commit must agree.
      Add a failing-first test that would count the second story with the current working-tree union.
- [ ] **A fact keeps the same day key across generation and commit.** Specify one date source that is
      knowable before the commit and remains the committed fact's date after midnight and when an old
      commit is amended without changing its author date. Add failing-first tests for both boundaries.
- [ ] Regenerate the report in the commit that closes this reopened story and prove `--check` in a
      clean checkout of that exact commit, not only in its originating worktree.

## Notes
- **Found while integrating `S-29`**, not by the suite, and found twice before it was understood: the
  first read was "main is red, regenerate it" (`cffb6ed`), which was true and which held only until the
  next story moved. The second read — after filing `S-30` re-broke it inside a commit that had verified
  green before committing — is that the check is asking for something unobtainable rather than that
  anyone forgot to run it.
- **This is the same shape as the bug `AGENTS.md`'s gate section is about**, one layer up. That section
  exists because a CI job ran something nothing local reconciled, and the fix was to make the gate
  unable to omit a CI job. Here the gate does run the job — and the job cannot pass, so the pressure is
  toward learning to ignore a red step. That is the failure mode `X-36` also found from the other end:
  a test that is green and asserts nothing. A step that is red and means nothing trains the same habit.
- **`docs/maturity.md` is fenced from implementors** by the coordination rules, which concentrates the
  breakage on whoever writes the ledger commit and hides it from the people whose stories cause it.
  Worth considering whether the trailing-regeneration dance should be written down in `AGENTS.md` as the
  interim workaround until this lands.
- Reads with `X-32`, which built the report and its check, and whose changelog entry already recorded
  that "closing `X-32`'s own story changed the answer and turned the gate red until the report was
  regenerated" — the defect was visible in the entry announcing the feature, described as the check
  earning itself.

## Progress

**Which of the three options this is: the third.** The day rows now come from a source that does not
move under the commit that writes them — committed history **union the working tree**
(`uncommitted_story_facts`, `scripts/maturity.py`). `git commit` only relocates a fact from the second
half of that union to the first, so the count is identical either side of it and the commit that files
or closes a story can carry a report of itself.

The other two options were considered and rejected for the same reason: **they move the flap rather
than removing it.** A day row that `--check` tolerates while it is today is strictly checked tomorrow,
so it goes red on some later commit that touched nothing at all — the same "red for no defect" the
story is about, fired at a more confusing moment. Rendering the day provisional has that problem too
unless it stops carrying numbers, and the crossover date is the number to watch, so the table keeps
its current-day row with real numbers in it.

What this cost: today's row is now a working answer rather than a purely historical one, and a dirty
tree reports a day git history does not show yet. The report says so, in a new bullet under *What this
cannot see*. A clean tree — CI, and every commit touching no story — contributes nothing to the union
and sees exactly the history-only answer it saw before, so `--check` is no more tolerant than it was:
`test_a_report_that_was_not_regenerated_is_still_red` and `test_an_edited_report_is_still_red` pin
that, and both were already green at the base.

**The recursion, handled rather than observed.** Closing this story writes `+status: done` into this
file, which is a `Closed` fact for the day of the closing commit. With the fix, `/track:done`'s own
regeneration sees that line in the working tree, counts it, and the commit is green — which is the
mechanism, exercised on itself. The base commit `36d0b3f` was already red for precisely this shape
(`Closed` read 15, history said 16, because `M-31`'s closing commit regenerated before committing);
`docs/maturity.md` is regenerated here and that red is gone.

**What an implementor will now see, which nobody had written down.** Filing a story in a worktree turns
the `maturity` step red *before* the commit, where previously the red arrived after it. Implementors are
fenced from `docs/maturity.md`, so they cannot clear it themselves; the ledger commit does. This is a
real consequence of reading the working tree and not a defect in it — the alternative was a step that
could never pass — but it moves when the red appears, and the next implementor should read that here
rather than discover it. If it becomes friction, the fix is a coordination one: either the fence opens
for this one generated file, or the coordinator regenerates on receipt.

**Three defects found in review, all in the first round's own tests rather than the fix.** Recorded
because the first two are the failure mode this story is about, arriving from new directions.

- *A test that could not fail, guarding the load-bearing line.* The zero-guard in `discovery_rate` is
  what keeps a clean tree from printing a phantom `| today | 0 | 0 | +0 |` row every midnight —
  `days` is the union of the counters' keys and `Counter[key] += 0` creates the key. Deleting the guard
  left all seven new tests green, including the one whose docstring named that exact property: its
  `assertNotIn` could not fire because every fixture files a story *today*, so today always had a row.
  That is the `X-36` shape inside the gate's own measuring instrument. Replaced with
  `test_a_day_with_no_story_activity_gets_no_row_at_all`, which back-dates the whole fixture history to
  2020 so today is genuinely empty. All three fixes are now mutation-tested: breaking each one turns
  exactly one named test red, and no test catches more than one.
- *A scratch file counted as a filed story.* `is_story` tested the file name, so an untracked
  `notes.md` under `docs/stories/` counted as filed. That made `--check` red on a tree whose report was
  right, and worse: green in the tree holding the scratch file and red on a clean checkout of the same
  commit, because the file never gets committed. Local green with CI red is the `X-22` class this
  repository's gate section exists to prevent. **Decided: a scratch file is not a story**, by the same
  test `stories()` applies, now factored into `story_fields` so there is one definition rather than
  two. Both halves of it are load-bearing — `_TEMPLATE.md` carries an `id:` of its own, so frontmatter
  alone would count the template.
- *Two readers for the closing line.* History matched `startswith("+status: done")`, the working tree
  matched equality, so a story filed already `done` with a trailing space was closed according to one
  half of the union and open according to the other — the flap surviving on malformed frontmatter.
  `closes_a_story` is the single reader now, tolerating trailing whitespace because the frontmatter
  parser does, which means the day row and the board agree about what is closed. `M-31`'s shape and
  `M-31`'s fix. Verified behaviour-preserving on real history: 121 closing lines, none malformed.

One asymmetry is left deliberately. Committed history hands the generator file *names* and no content,
so `Filed` for a past day is decided by name while the working-tree half reads content. Closing it
costs a `git show` per historical addition — 154 of them, about seven seconds on every gate run — to
guard a case that requires committing junk into the board directory. Instead
`test_the_name_rule_and_the_story_rule_agree_on_the_real_board` holds the two rules equal on every file
the board currently has; they agree across all 154 additions in history, the only disagreement being
`_TEMPLATE.md`, which both exclude.

**What the fourth Acceptance tick does and does not claim.** The `maturity` step no longer goes red
without a defect, which was this story's share of predicate 3. The predicate itself still reads `open`,
waiting on `X-40` and `X-41` — a test that fails when the machine is busy, and a step that prints a
defect and exits 0 — and that is the honest reading rather than a gap here. The premise the item was
written on ("`maturity.md` reports it as met") is already gone: `X-42` moved the association into story
frontmatter, so predicate 3 went `open` the moment these three were filed against it.

Left for the coordinator: the story's Notes float writing the trailing-regeneration dance into
`AGENTS.md` as an interim workaround. Deliberately not done — the workaround is what this removes, and
`AGENTS.md` is outside this story's write set.

### Reopened by the 2026-07-30 repository review

The working-tree union fixes the ordinary all-changes commit, but it does not yet establish the
snapshot invariant this story needs. `uncommitted_story_facts` reads the complete index, working tree
and untracked story set, so a selectively staged report can include a story absent from the commit.
It also assigns every pending fact the wall-clock day, while history groups facts by the commit's
author date. A midnight boundary or an amended older commit can therefore move a fact between rows.

These are R-03 and R-04 in
`docs/reviews/2026-07-30T07-50-49+02-00-repository-review.md`. They are extensions of X-39's contract,
not new backlog items, so the story returns to `ready` until the selective-snapshot and stable-date
acceptance cases are satisfied.
