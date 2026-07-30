---
id: X-42
title: Stop a predicate reporting met while open stories describe it failing
pillar: Build
status: ready
priority: 2
design: docs/roadmap.md
epic: conformance
areas: [docs]
note: `scripts/maturity.py:100` hardcodes predicate 3's story list as X-28/X-29/X-34/X-36, all done, so it computes as met while X-39, X-40 and X-41 — all filed for that predicate — are open and invisible to it
---

# Stop a predicate reporting met while open stories describe it failing

## Goal
Make a predicate's reported state account for every story filed against it, so that filing a defect
against a predicate cannot leave the report claiming that predicate is met.

## Acceptance
- [ ] **Predicate 3 reports its real state.** `scripts/maturity.py:100` reads
      `Predicate(3, "A red gate means a defect", "computed", ["X-28", "X-29", "X-34", "X-36"])`. All
      four are `done`, so it computes as **met** — while `X-39`, `X-40` and `X-41` are open and each
      describes that predicate failing: a gate step that cannot pass, a test that fails because the
      machine was busy, and a step that prints a defect and exits 0. Add them, and the predicate reads
      `open` until they close.
- [ ] **The list stops being the thing a filer has to remember.** The report's own text says *"a
      predicate is met when every story named for it is `done` — the stories are the definition, so
      this table cannot drift from the board"*. That holds only while the mapping is maintained by hand,
      and it was not: three stories were filed for predicate 3 in one session and none of them was
      added. Decide how the association is carried — a `predicate:` field in story frontmatter, so the
      story declares it and the report reads it, is the obvious candidate — and say why the chosen
      answer cannot rot the same way.
- [ ] **Every predicate's list is audited, not just 3.** The same hand-maintenance applies to 1, 2, 5
      and 7. Check each against the board for stories filed against it and never wired in, and report
      what you find; a second stale list would change this story from a fix into a pattern.
- [ ] **Failing-first test:** `scripts/test-maturity.py` gains a case asserting that a predicate with an
      open story naming it does not report met. It must fail before the fix — which it will, because
      that is exactly today's state with predicate 3.
- [ ] **The report says how a predicate's stories are determined**, wherever it explains the other
      caveats. A reader who trusts a `met` needs to know what could make it wrong.

## Notes
- **Found while answering "how far are we from a release".** The answer turned on predicate 3, and
  checking it rather than reading the table off the page is what surfaced this. That is the whole
  argument for the story: the table is linked from the README as a measurement, and the measurement was
  about to report the alpha complete.
- **It would have reported the alpha met.** `X-38` closes predicate 1, the last one the table showed
  open. The moment its story flips to `done`, this table reads **7 of 7** — with predicate 3 false in
  three known ways. `1.0.0-alpha` would have been cuttable on the strength of it.
- **This is the fourth instance of one meta-defect in a single session, and that is the finding.**
  `X-39`: the report cannot be green in the commit that moves a story. `X-40`: a test fails when the
  machine is busy. `X-41`: a gate step prints a defect and exits 0. And this: a predicate cannot see the
  stories filed against it. Each is a measurement that does not survive contact with the thing it
  measures. Priority 2 rather than 3 because this one hides the other three.
- **Predicate 3 is load-bearing.** `docs/roadmap.md` says so explicitly — every predicate is asserted
  by the gate, so a gate that cries wolf invalidates all of them. A predicate that cannot see its own
  defects is worse than one known to be open.
- Reads with `X-32`, which built the report and whose changelog entry already recorded the check
  "earning itself" when closing its own story turned the gate red — the same class of self-reference,
  noticed and left in place.
