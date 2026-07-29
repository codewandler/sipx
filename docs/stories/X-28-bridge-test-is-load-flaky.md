---
id: X-28
title: Make the bridge audio test deterministic under load
pillar: Build
status: ready
priority: 4
design: docs/designs/media.md
epic: conformance
areas: [sipx-media, tests]
note: found by M-25 — it races play against a fixed 400ms record on real sockets and records zero samples under load, so it will be blamed on innocent diffs
---

# Make the bridge audio test deterministic under load

## Goal
Stop `audio_played_into_one_call_is_heard_on_the_other` failing for reasons that have nothing to do
with the change under test, so a red gate means what it says.

## Acceptance
- [ ] `crates/sipx-media/tests/bridge.rs::audio_played_into_one_call_is_heard_on_the_other` passes
      deterministically while the machine is loaded — concretely, while several other gates are
      compiling concurrently, which is the condition under which it was observed to fail.
- [ ] The failure mode is understood and named before it is fixed. It records **zero of 3200
      samples**, not a degraded count, which is a different thing from "a bit slow" and the story
      should say which of the two it actually is.
- [ ] The fix does not weaken what the test asserts. Loosening the sample threshold until it passes
      would leave a test that no longer proves audio crossed the bridge — the point of it.
- [ ] Any sibling test racing a fixed wall-clock duration against real-socket work is found by the
      same sweep and named, fixed or explicitly left with a reason. `record_until_idle(400ms)`
      against `play` is a shape, not a one-off.
- [ ] Failing-first evidence: the test failing under artificial load, quoted from a real run.

## Progress
- Not started.

## Notes
- Found by `M-25` during a gate run with several worktrees compiling concurrently. It passed 3/3
  standalone immediately afterwards and stayed green in every subsequent run, which is what makes
  it dangerous: **it will be blamed on whichever diff happens to be in flight.** `M-25` had to
  prove it was not its own change by showing `bridge.rs` contains no reference to `srtp`, `dtls` or
  `Srtp` at all.
- This is a real-socket, wall-clock test: `play` racing `record_until_idle(400ms)`. Under load the
  recorder's idle window elapses before any audio arrives.
- **Priority 4 because a flaky gate step is worse than a missing one.** A test that fails at random
  trains everyone to re-run the gate instead of reading it, which is how a genuine regression gets
  waved through — and this project has already paid once for a CI signal nobody was watching (see
  `AGENTS.md` on the MSRV job that was red through two releases).
