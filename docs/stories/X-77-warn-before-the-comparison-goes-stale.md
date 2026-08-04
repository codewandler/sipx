---
id: X-77
title: Warn before the comparison goes stale
pillar: Build
status: done
priority: 19
design: docs/designs/stack-comparison.md
epic: stack-comparison
areas: [scripts, docs]
predicate:
announcement:
note: every observation expires on the same day · a wall with no notice is the failure people learn to silence
---

# Warn before the comparison goes stale

## Goal

Give the staleness gate a notice period and a standing countdown, so refreshing the comparison is
something somebody chooses to do rather than something a release run discovers.

## Acceptance

- [ ] `STALE_WARNING_DAYS = 30` is a module constant in `scripts/comparison-report.py`, argued in a
      comment naming this story and the reason.
- [ ] `expiring_soon(observation_list, today)` returns one line per observation inside the band, and
      is **not** called from `check()` — `check()` returns failures, and a warning folded into it
      would either fail the build early or teach a reader that its return value is advisory.
- [ ] `main()` prints those warnings to stderr in both modes; the exit code is unaffected.
- [ ] The success line carries a countdown on every green run, not only near the limit:
      `comparison: N stacks over M dimensions, every claim evidenced, none stale (next expires in D days)`.
- [ ] `staleness_problems()` is unchanged: past the limit is still a hard failure naming
      `REFRESH_COMMAND`. The band must not have replaced the wall, and the existing tests proving the
      wall must still pass.
- [ ] A `TestCase` in `scripts/test-comparison-report.py` with all four kinds: an observation inside
      the band warns **and** leaves `check()` empty · one outside the band does not warn · one past
      the limit still fails and still names the refresh command · the countdown reaches the printed
      line.
- [ ] `docs/comparison/README.md` states the band in `### Staleness is a failure, not a footnote` —
      what warns, what fails, and why they are different. The `## What is checked` list is a list of
      failures and stays one.
- [ ] The rendered preamble in `docs/comparison.md` states the notice period, changed **in the
      renderer** and regenerated, never hand-edited.
- [ ] `.claude/skills/compare-stacks/SKILL.md` gains an **Adding a subject** section, and says that
      divergent `evaluated_at` dates are the desired end state rather than untidiness.
- [ ] Both directions of the band demonstrated and recorded in Progress.
- [ ] `./scripts/gate.py` green.

## Progress

Implemented 2026-08-04. Everything in Acceptance is satisfied.

- **`STALE_WARNING_DAYS = 30`** in `scripts/comparison-report.py`, argued in a comment naming the
  story: `--check` was green until the day it was not, and the first dataset was derived in one
  sitting so every observation in it expires together.
- **`expiring_soon(observation_list, today)`** and **`days_until_expiry(...)`** added beside
  `staleness_problems()`, both built on a shared `_expiry_days()` that returns `None` for a marker
  (nobody looked, and that does not age) and for a missing or malformed date (which is
  `staleness_problems()`' business, not this one's). Neither is called from `check()`.
- **`main()` prints `notice:` lines to stderr before the verdict**, in both modes, and the exit code
  is untouched.
- **The success line carries a standing countdown**:
  `comparison: 8 stacks over 6 dimensions, every claim evidenced, none stale (next expires in 180 days)`.
- **Seven tests added** (55 total, all passing), including `test_the_band_did_not_replace_the_wall`
  — the failure mode of adding a warning is that it quietly becomes the only thing that happens.
  `test_a_marker_is_never_warned_about` pins the marker case.

**Both directions demonstrated on the real dataset**, by moving one observation's `evaluated_at` and
reverting. **The subject's id is redacted below as `<subject>`**: this file is a story, not one of
the three artifacts `COMPARISON_SCOPE` covers, so the real transcript cannot be pasted here. The
pre-commit hook caught exactly that on the first attempt to commit this story, which is the boundary
working rather than an inconvenience.

```
# 165 days old — inside the band
notice: <subject>/language-safety expires in 15 days. re-derive it with the compare-stacks skill, then ./scripts/comparison-report.py
comparison: 8 stacks over 6 dimensions, every claim evidenced, none stale (next expires in 15 days)
exit=0

# 181 days old — past the limit
Comparison claims the evidence does not back up:
  <subject>/language-safety is stale: evaluated 181 days ago, and the limit is 180. re-derive it with the compare-stacks skill, then ./scripts/comparison-report.py
exit=1
```

Note the second case prints **no** notice — past the band it is a failure, not a warning, and the
two never both fire for one row.

- **`docs/comparison/README.md`** gained the band in `### Staleness is a failure, not a footnote`,
  including why `check()` does not carry it and why the shared expiry date is a state to grow out
  of rather than preserve. The `## What is checked` list is untouched and remains a list of
  failures.
- **The rendered preamble** was changed in `render()` and the document regenerated, never
  hand-edited.
- **`SKILL.md`** gained an **Adding a subject** section — the procedure documented refreshing and
  silently assumed `stacks.json` was populated — plus the rule that divergent dates are the desired
  end state and that `evaluated_at` is never edited to achieve them.

## Notes
- All 48 observations in the first dataset carry `evaluated_at: 2026-08-04`, so they expire together
  on **2027-01-31**. The design anticipated "a red gate on a date" and named it the failure most
  likely to be silenced; it did not anticipate that the date is all-or-nothing.
- **No artificial staggering.** `evaluated_at` is when the evidence was read, and moving it to smooth
  the cliff would be a lie told to a checker. Dates diverge on their own once subjects are refreshed
  one at a time, which is what the band exists to enable.
- This is also what makes a larger subject set survivable. Browser/JS, Python and Erlang/Elixir are
  wanted and deferred until one refresh cycle has been run.
