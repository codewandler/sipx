---
id: X-118
title: Make bounded-timeout tests robust under load
pillar: Build
status: ready
priority: 31
design:
epic: test-surfaces
areas: [sipx-cli, ci]
predicate:
announcement:
note: a CANCEL test that passes in isolation timed out under three concurrent gate runs · a flaky red is worse than a slow one
---

# Make bounded-timeout tests robust under load

## Goal

Stop tests that assert a bounded wall-clock from failing because the machine was busy, without
weakening what they assert.

## Acceptance

- [ ] `interrupting_a_pending_dial_cancels_without_manufacturing_a_bye` and every sibling asserting
      a wall-clock bound either use a controllable clock or state a tolerance derived from the
      machine, not a fixed number tuned on an idle box.
- [ ] A failing-first proof runs the suite under deliberate CPU contention and shows the assertion
      surviving, while a genuinely unbounded operation still fails it.
- [ ] `check-fixed-sleep.py` stays green — this must not become a sleep.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `M-41`'s adjacent findings. Its intermediate gate run hit `test: exit 101`
  on that CANCEL test while three gates ran concurrently; the test passes in isolation and the final
  run on a quiet box had all 145 suites green. `M-41`'s diff touches no `sipx-cli` file.

- 2026-08-08: **stronger evidence, and a corrected cause.** The box this was observed on is shared:
  alongside seven sipx implementor worktrees, five other checkouts (`flux`, `flux-c728`, `flux-c736`,
  `flux-c740`, `flux-c742`) were running their own builds and gates, at load average **41.85 on 20
  cores**. So the CANCEL timeout was not "three concurrent sipx gates" — it was an oversubscribed
  machine, which is both more likely to recur and entirely outside this repository's control. A
  wall-clock assertion tuned on an idle box is not a property of the code under test.

- 2026-08-08: **a deterministic cause has been found for the same symptom, and it was never ruled
  out against the load diagnosis.** `X-121` traced it: `check-cli-reference.py` builds `sipx-cli`
  with default features, and `gate.py` runs that step *after* the all-features `test` step, so every
  gate run leaves a binary the next run's process tests spawn — failing as "heard no audio at all",
  which is exactly what this story attributes to contention. `X-124` owns the fix and `X-121`'s
  guard now makes the two distinguishable. **Do this story after those**, and re-measure whether a
  load-sensitive assertion remains once the deterministic cause is gone; it may be that little or
  none of what was attributed to load was load.

## Notes

- This matters more now: concurrent implementors are the normal working mode, so a load-sensitive
  test produces reds nobody caused and everybody has to investigate.
