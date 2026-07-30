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
- Not started. Filed while integrating the `X-39`/`X-40`/`M-33` wave, from three independent sightings.

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
