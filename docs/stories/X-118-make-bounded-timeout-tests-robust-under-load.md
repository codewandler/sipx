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

## Notes

- This matters more now: concurrent implementors are the normal working mode, so a load-sensitive
  test produces reds nobody caused and everybody has to investigate.
