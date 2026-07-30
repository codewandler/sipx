---
id: X-44
title: Guard the fixed-sleep rule mechanically, because the sweep did not hold
pillar: Build
status: ready
priority: 3
design: docs/designs/media.md
epic: conformance
areas: [scripts, sipx-media, sipx-cli]
note: found while integrating the X-39/X-40/M-33 wave — `X-29` swept the workspace and `0.12.0` claims "no test in the workspace now asserts after a fixed sleep", but nothing enforces it and two new instances appeared within the same wave
---

# Guard the fixed-sleep rule mechanically, because the sweep did not hold

## Goal
Make a new fixed-sleep assertion fail the gate, so the rule `docs/designs/media.md` already states
normatively stops depending on the next person to sweep for violations.

## Acceptance
- [ ] **The claim currently has nothing behind it.** `CHANGELOG.md`'s `0.12.0` entry for `X-29` says
      "No test in the workspace now asserts after a fixed sleep", and `X-28` cleared the media path
      before it. There is no gate step, no script and no lint asserting that property, so the claim was
      true at the moment it was written and nothing keeps it true. Reproduce by adding a test that
      sleeps and then asserts on a real-time side effect, and observing a green gate.
- [ ] **Two instances appeared inside one wave, which is the argument for the story.**
      `crates/sipx-cli/tests/interop_media/mod.rs:105` gave the first echoed packet and the
      inter-packet gaps the same 600 ms window (found by `X-40`), and `M-33` landed two interval
      assertions that failed **14 of 20 runs** at 6× CPU oversubscription (0 of 12 at 2×, 1 of 8 at 5×).
      Both were caught by a human reading a diff, which is exactly the mechanism this repo replaces with
      checks elsewhere.
- [ ] **The rule is already written; make it executable.** `docs/designs/media.md:213-216`: "a fixed
      wall-clock duration may bound a failure, or define silence. It may not stand in for a
      happens-before." The guard implements *that* sentence, and the design doc becomes the normative
      reference rather than a restatement.
- [ ] **The three legitimate categories keep passing, and are named at their sites.** `X-29` identified
      them: a duration that **bounds a failure** (a timeout), one that **defines silence** (an idle
      window — note `X-40` proved an idle window must not also be a start deadline), and one where the
      **clock is itself the measurement** (`crates/sipx-cli/tests/cli.rs:719`'s `elapsed() < 12s`
      separating 3 s from 32 s, where load can only fail it, never pass it wrongly). Each surviving
      sleep carries its reason **at the call site**, not in a suppression list — same standard as
      `X-35`'s claims guard, which was explicitly built without one "under any name".
- [ ] **The guard cannot be satisfied by a rename.** State how it identifies the shape rather than the
      spelling: `sleep` is one spelling, and `tokio::time::sleep`, `std::thread::sleep`,
      `advance`, a hand-rolled deadline loop and a bare `interval.tick()` are others. A guard that only
      greps `sleep(` invites the next author to write the same defect differently — the
      "rule fitted to the data it was tested on" failure this repo keeps warning about.
- [ ] **`gate.py --check` accounts for the new step**, and the step is a CI job or declared CI-only
      with a reason, per the property `X-22` established.
- [ ] Failing-first test: extend `scripts/test-gate.py` with a case that plants a fixed-sleep assertion
      in a fixture tree and requires the guard to refuse it, plus a case asserting each of the three
      legitimate categories is *not* refused. Both must be red before the guard exists.

## Progress
- Filed while integrating the `X-39`/`X-40`/`M-33` wave, from three independent sightings.
- `scripts/check-fixed-sleep.py` is the guard, gate step `fixed sleeps` (23 steps now), CI job
  `fixed-sleep`. It reads the **shape** — a suspension whose only completion condition is a clock
  reading a fixed duration — so `tokio::time::sleep`, `std::thread::sleep`, `sleep_until`,
  `time::advance`, a bare `interval.tick()`, a hand-rolled `while now < deadline` spin and a private
  helper wrapping any of them are one thing. The wrapper hop is what a grep can never see, and
  `scripts/test-gate.py`'s `test_a_rename_does_not_get_past_it` plants the identical defect in all
  six spellings and requires all six to be refused.
- **A second rule catches the other regression, and a sleep-grep never could.** A loop whose every
  pass is bounded by a *relative* `timeout(D, …)` spends one duration on both "how long may it take
  to start" and "how long a gap means it has ended" — `X-40` exactly, and `M-34` one call along.
  Run against the tree as it stood at `882fc5f^`, the guard names
  `crates/sipx-cli/tests/interop_media/mod.rs:112`, which is the defect at its own line. There is no
  `sleep` in it.
- **Structural excuses, not written ones.** A duration bounding an awaited event (`timeout`, a
  `select!` arm beside another arm), a poll interval inside a loop that re-tests its condition, an
  absolute `timeout_at` bounding a whole loop, and a window the loop body reassigns after the first
  arrival — which is `X-40`'s own fix — are not reported, because those are the fixes the rule asks
  for and a guard that charged for its own remedy would be switched off. A `start_paused` runtime is
  exempt for the same reason: there is no wall clock to race.
- **Scope: `src/` as well as `tests/`.** `X-40` proved the defect can be written in production code,
  and `crates/sipx-media/src/session.rs` holds more fixed-duration waits than any file under a
  `tests/` directory — a guard over test directories would have covered less than half the suite.
  Seven of the thirty hits were in `src/`, and two of those were the defect.
- **The sweep found thirty sites and the `0.12.0` claim was false.** Exactly two sites in the whole
  workspace already said which question their duration answered — `cli.rs`'s `elapsed() < 12s` and
  `events.rs`'s widened window, both left deliberately by `X-29` — and the other thirty said
  nothing. Two of those were the defect and are now causal waits: `session.rs`'s
  `media_returns_to_where_it_came_from_not_where_the_sdp_said` and
  `audio_flows_in_both_directions_at_once` both slept for a fixed window to let the far end latch a
  source address, and now record the packet that latches it. The rest were classified at the call
  site.
- **The load gradient the Notes ask about was considered and not adopted.** Running the suspect
  tests under deliberate oversubscription would manufacture CPU load, which non-negotiable 5 in
  `AGENTS.md` forbids; and `M-33`'s own numbers are the argument against it as a *check* — 14 of 20
  at 6x, 1 of 8 at 5x, 0 of 12 at 2x. A detector that fires probabilistically makes a green run
  prove nothing and a red one a re-run, which is precisely the "re-run it instead of believing it"
  disease `X-34` was filed against. The static check is deterministic, costs milliseconds, and
  fails the same way every time. The load gradient stays what it was: how a *defect* is measured
  once it is suspected, not how it is found.
- **No suppression list, under any name** (`X-35`'s standard). No path is exempt; the four
  categories a duration may claim are `a bound on failure`, `a definition of silence`, `the clock is
  the measurement` and `ordering a stimulus`. The fourth is the one to argue with in review — it is
  the case `X-28` and `X-29` both left sites for and neither named, and the question a reader should
  put to it is "what would you have waited for".

## Notes
- **Reads with `X-40`**, which is the third instance of the family and proved the defect can live in
  *production* code rather than the test — `record_until_idle`'s single window was both a start deadline
  and an end-of-stream gap. A guard over tests alone would not have caught that, so decide explicitly
  whether the rule applies to `src/` as well, and say why.
- **Reads with `M-33`'s review**, which supplied the load gradient. A guard that runs only at 1× load
  proves nothing; consider whether the gate should run the suspect tests under deliberate
  oversubscription instead of, or as well as, static analysis.
- **Prior art in this repo for the mechanism**: `scripts/check-provenance.sh`,
  `scripts/check-audio-claims.py`, `scripts/check-app-surface.py`, `scripts/check-pool-key.py`. All four
  exist because a one-time cleanup did not stay clean. `X-26` and `X-35` are the cautionary pair —
  `X-35` found `X-26`'s guard passing with a phantom claim in place because the guard read three strings
  and the README's crate table was not one of them.
- The `0.12.0` changelog entry for `X-29` should be read as scoped to what it swept, not as a standing
  property, until this story lands.
