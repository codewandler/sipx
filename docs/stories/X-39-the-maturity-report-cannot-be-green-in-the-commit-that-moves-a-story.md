---
id: X-39
title: Stop the maturity report from making the gate red for something that is not a defect
pillar: Build
status: ready
priority: 3
design: docs/roadmap.md
epic: conformance
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
- [ ] **A commit that files or closes a story can be gate-green without a second commit.** Reproduce
      first: on a clean tree, add a story file, run `./scripts/maturity.py`, commit both, then run
      `./scripts/maturity.py --check` — it exits 1, and the diff is one number in the *Discovery versus
      closure* table's current-day row. The cause is that `Filed` comes from
      `git log --diff-filter=A -- docs/stories` and `Closed` from scanning committed diffs for a
      `status: done` line (`scripts/maturity.py:160,175`), so the count the report must contain is
      created *by the commit that contains the report*. No ordering of `maturity.py` and `git commit`
      can satisfy it, which is why it was regenerated twice on 2026-07-30 — `cffb6ed` and `4014b95` —
      with no defect either time.
- [ ] **The fix does not stop the table measuring what it measures.** Discovery-versus-closure is the
      one place the report says something the board cannot, and `X-32` added it deliberately: burn-down
      is not a maturity signal while discovery outpaces closure. Deleting the current-day row, or the
      table, is not the answer — the crossover date is the number to watch. State which of these it is
      and why: the check tolerates the in-flight day; the report renders that day as provisional; or
      the day rows come from somewhere that does not move under the commit that writes them.
- [ ] **`--check` still catches the drift it was built to catch.** It has earned itself twice: on its
      first run (`X-32`, recorded in the changelog) and on 2026-07-30, when `main` was found red for
      real because closing `S-25` moved the board aggregates and nothing re-ran the script. A fix that
      makes the check tolerant enough to miss *that* is worse than the flapping, because the
      report is linked as a measurement.
- [ ] **Alpha predicate 3 is honest again.** The predicate is "a red gate means a defect", and
      `maturity.md` reports it as met while the gate's own `maturity` step is red for no defect. The
      predicate is documented as load-bearing for every other predicate — all of them are asserted by
      the gate — so a gate that cries wolf here weakens the other six. Note the recursion and handle
      it: this story closing will itself move the table.
- [ ] Failing-first test: extend `test-maturity` with a case that files a story, regenerates, commits
      and asserts `--check` is green. Name it. It must fail before the fix.

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
