---
id: X-42
title: Stop a predicate reporting met while open stories describe it failing
pillar: Build
status: in-progress
priority: 2
design: docs/roadmap.md
epic: conformance
areas: [docs]
predicate: 3
note: `scripts/maturity.py:100` hardcodes predicate 3's story list as X-28/X-29/X-34/X-36, all done, so it computes as met while X-39, X-40 and X-41 — all filed for that predicate — are open and invisible to it
---

# Stop a predicate reporting met while open stories describe it failing

## Goal
Make a predicate's reported state account for every story filed against it, so that filing a defect
against a predicate cannot leave the report claiming that predicate is met.

## Acceptance
- [x] **Predicate 3 reports its real state.** `scripts/maturity.py:100` reads
      `Predicate(3, "A red gate means a defect", "computed", ["X-28", "X-29", "X-34", "X-36"])`. All
      four are `done`, so it computes as **met** — while `X-39`, `X-40` and `X-41` are open and each
      describes that predicate failing: a gate step that cannot pass, a test that fails because the
      machine was busy, and a step that prints a defect and exits 0. Add them, and the predicate reads
      `open` until they close.
- [x] **The list stops being the thing a filer has to remember.** The report's own text says *"a
      predicate is met when every story named for it is `done` — the stories are the definition, so
      this table cannot drift from the board"*. That holds only while the mapping is maintained by hand,
      and it was not: three stories were filed for predicate 3 in one session and none of them was
      added. Decide how the association is carried — a `predicate:` field in story frontmatter, so the
      story declares it and the report reads it, is the obvious candidate — and say why the chosen
      answer cannot rot the same way.
- [x] **Every predicate's list is audited, not just 3.** The same hand-maintenance applies to 1, 2, 5
      and 7. Check each against the board for stories filed against it and never wired in, and report
      what you find; a second stale list would change this story from a fix into a pattern.
- [x] **Failing-first test:** `scripts/test-maturity.py` gains a case asserting that a predicate with an
      open story naming it does not report met. It must fail before the fix — which it will, because
      that is exactly today's state with predicate 3.
- [x] **The report says how a predicate's stories are determined**, wherever it explains the other
      caveats. A reader who trusts a `met` needs to know what could make it wrong.

## Progress
- **The list is gone, not extended.** `Predicate` no longer carries stories at all. `scripts/maturity.py`
  reads a `predicate:` frontmatter field off each story (`predicate_stories`, `story_predicates`), and a
  predicate is open while any story declaring it is open. Adding the three missing IDs to the literal
  would have fixed today and guaranteed the repeat, which is what the Acceptance refused.
- **Why it cannot rot the same way.** There is exactly one place the association is recorded and it is
  the file the filer is already writing. Three mechanical consequences: a `predicate:` naming a
  predicate the roadmap does not have exits non-zero with a diagnostic rather than being dropped
  (`test_a_story_declaring_a_predicate_that_does_not_exist_is_an_error`); a malformed value does the
  same rather than raising (`test_a_predicate_field_that_is_not_a_number_is_an_error`); and a *computed*
  predicate no story declares reads **unknown**, never `met`, so wiping declarations cannot manufacture
  a met (`test_a_computed_predicate_no_story_declares_is_unknown_not_met`). A real-board test asserts
  every computed predicate is actually declared by something, so the mechanism cannot be merely
  available.
- **The residual risk, stated because the report states it too.** A filer who leaves `predicate:` empty.
  No script can decide which predicate a story *should* have named, so that one cannot be closed — but
  it is strictly narrower than a Python literal the filer had no reason to open. The template and
  `AGENTS.md` name the field; `docs/maturity.md`'s "What this cannot see" says this is the remaining
  hole.
- **A story may declare two predicates** (`predicate: [3, 7]`). Forcing one choice would leave the other
  reading `met`, which is this story's defect in miniature.
- **Frontmatter wired into 19 stories**, which is the whole of the previous hardcoded mapping plus the
  three omissions plus `X-35`: predicate 1 → `X-30`, `X-33`, `X-35`, `X-37`, `X-38`; 2 → `X-19`, `X-31`;
  3 → `X-28`, `X-29`, `X-34`, `X-36`, `X-39`, `X-40`, `X-41`, `X-42`; 4 → `S-27`, `P-7`; 5 → `A-8`;
  7 → `X-32`. Predicate 6 is declared by nothing and stays `met (attested)`.
- **The audit found one further omission and no second stale list.** Predicate 1's list omitted `X-35`,
  whose own Notes open *"This is alpha predicate 1 at the layer the predicate does not currently
  reach"* — the same defect as predicate 3's, and invisible only because `X-35` closed. It is wired in
  now. Predicates 2, 5 and 7 are clean against the board. Two near-misses were deliberately **not**
  wired: `S-30` and `S-32` say they bear on predicate 6 while stating in their own words that *"the
  predicate's letter holds while its spirit fails"*, so they do not falsify it; and `M-32` cites
  predicate 3 as a constraint on how it must write its own tests, not as a defect against it.
- **`docs/maturity.md` is fenced for this story and deliberately not committed.** The change makes
  predicate 3 read `open`, waiting on `X-39`, `X-40`, `X-41`, `X-42`, and the total go 6 → 5 of 7. Run
  `./scripts/maturity.py` to apply it; until then the `maturity` and `maturity tests` gate steps are red
  on that file alone.
- **`X-39` is a neighbour and this makes it no harder.** The Filed/Closed rows are untouched by this
  change, so the drift here is content, not the self-reference `X-39` is about. Nothing added here reads
  git.

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
