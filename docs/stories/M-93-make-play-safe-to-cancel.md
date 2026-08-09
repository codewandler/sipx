---
id: M-93
title: Make a cancelled play safe to tear down
pillar: Media
status: ready
priority: 8
design:
epic: media
areas: [sipx-media]
predicate:
announcement:
note: aborting a task parked in MediaSession::play wedges hang_up · ten calls took over 90 s to tear down, four without
---

# Make a cancelled play safe to tear down

## Goal

Let a caller drop a task that is inside `MediaSession::play` without stalling the call's teardown.

## Why

Measured while writing `crates/sipx-call/examples/capacity_test.rs`, which holds calls up and plays
a tone into each for the life of the run. The obvious shutdown — `JoinHandle::abort()` on every tone
task, then `hang_up()` on every call — does not work: with ten calls the process had not finished
tearing down after **90 seconds**, and the same run stopping the tones *between* frames instead
finished in **four**. The difference is whether the aborted task was parked inside `play` at the
moment it was dropped.

So a future dropped mid-`play` leaves something behind that the teardown path then waits on. The
example works around it by checking a flag between plays and never aborting, which is a reasonable
thing for an example to do and not a reasonable thing to require of every caller: `abort()` on a
spawned task is the ordinary way to stop background work in this runtime, and a media API that
cannot survive it will be found the hard way by whoever writes the first real application.

This is not the same as the shed/queue behaviour `M-57` bounds, and not the same as the discard
accounting in `docs/specs/media-runtime.md` §4 — nothing is discarded here. Something is waited for.

## Acceptance

- [ ] The cause is identified and named — which resource a dropped `play` leaves held, and what in
      the teardown path waits on it — rather than papered over with a timeout.
- [ ] A failing-first test spawns a task playing into a session, aborts it mid-play, then tears the
      session down under a bound that the current behaviour exceeds.
- [ ] Either `play` becomes cancellation-safe, or its rustdoc states plainly that it is not, says
      what a caller must do instead, and the teardown path stops waiting unboundedly on it. A
      documented hazard is an acceptable answer; an undocumented one is not.
- [ ] Whatever is decided holds for the other long-running media entry points a caller might park a
      task in — `record_at_least`, `record_until_idle`, `recv` — or the difference is stated.
- [ ] `capacity_test`'s between-frames workaround is revisited: kept with a reason, or dropped
      because it is no longer needed.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-09: filed from a measurement rather than a reading. `--media idle` (no tones) tears down
  ten calls in 4 s; `--media full` with `abort()` did not finish in 90 s; `--media full` stopping
  between frames does it in 6 s. The example carries the workaround and the comment explaining it.
