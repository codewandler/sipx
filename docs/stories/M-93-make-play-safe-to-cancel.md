---
id: M-93
title: Make a cancelled play safe to tear down
pillar: Media
status: done
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

**Rewritten at closure, because the measurement refuted the premise these rows were written on.**
They asked which resource a *dropped* `play` leaves held; the answer is none — dropping one is safe
and always was. What could not be cancelled was a loop that had stopped suspending. The original
rows are preserved in the git history of this file; these are the ones that were actually met.

- [x] The cause is identified and named: once a session stops, every long-running media wait answers
      without suspending, so a loop over one holds a **runtime worker** the scheduler never gets
      back. `abort()` cannot land on a task that never reaches a suspension point, `JoinHandle::await`
      never returns, and dropping the runtime blocks. That is what the teardown was waiting on.
- [x] A failing-first test covers the shape that actually fails —
      `crates/sipx-media/tests/play_cancellation.rs`, whose
      `a_task_playing_when_its_call_ended_can_still_be_aborted` is red without the fix. Each test
      forces its runtime down rather than dropping it, because an ordinary drop *hangs* on this
      defect instead of reporting it, and a test that hangs says nothing.
- [x] The waits reach a suspension point on the paths that answer permanently, and their rustdoc
      states which loop is safe to write and which burns a core. Dropping a `play` is documented as
      safe and sufficient.
- [x] The sibling entry points are settled in the same terms: `recv` says
      `while let Some(frame) = ..` is the loop to write and `loop { recv().await; }` is the one that
      burns a core after the call ends; `record_at_least` and `record_until_idle` carry the same
      guarantee plus their own, different hazard — a dropped recording loses what it had collected,
      because the samples live in the future rather than in the session.
- [x] `capacity_test`'s workaround is revisited and **kept**, with the reason recorded at the site:
      the API is safe to abort now, and asking a loop to stop is still the clearer thing for a
      sample to demonstrate.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-09: filed from a measurement rather than a reading. `--media idle` (no tones) tears down
  ten calls in 4 s; `--media full` with `abort()` did not finish in 90 s; `--media full` stopping
  between frames does it in 6 s. The example carries the workaround and the comment explaining it.

- 2026-08-09: **not merged into `1.0.0-rc.13`, and the branch is where the work is.** The implementor
  was ended by an auth failure rather than by anything about the work; its worktree is preserved on
  `impl/M-93` as a WIP commit that asserts no acceptance row.

  What it established is worth more than what it left unfinished, because **it refutes this story's
  own hypothesis and the coordinator's public statement of it**. Aborting a task parked *inside*
  `play` was never the problem: the clip stops, its queued packets are discarded, and the session
  tears down in milliseconds. What wedges is the other order — a call ending underneath a loop that
  is still playing. Once the session has stopped, every long-running entry point answers immediately
  *without suspending*, so `loop { play().await }` stops being paced and becomes a bare busy loop; a
  task that never reaches a suspension point cannot be cancelled at all, `abort()` never lands,
  `JoinHandle::await` never returns, and dropping the runtime blocks. **The resource held is a
  runtime worker**, and that is what the teardown was really waiting on.

  The fix in progress adds a `yield_now` hop on the paths that can answer without parking. It works
  — `crates/sipx-media/tests/play_cancellation.rs` passes 3 of 3 — but it **regresses**
  `sipx-call`'s `every_playback_reports_its_own_end_by_id`, which passes on `main` and fails
  deterministically on the branch. The implementor had already named the cause in its last words:
  the hop fires on the `Completed` path the `C-3` watchers observe, and two watchers that both park
  before their clips end must be woken in the order the clips ended. It judged that a sharper rule
  was available — hop only where the wait genuinely could not have parked — and did not get to
  write it.

  So the remaining work is exactly that rule, plus proof that the ordering test goes green with the
  cancellation tests still passing. Do not merge before both hold.

- 2026-08-09: **the sharper rule is written, and both properties hold.** The hop now turns on the
  *answer* rather than on whether the wait parked. `PlaybackEnd::SessionEnded` is the permanent
  one — it answers at once and answers at once for ever, and `play` is `start` plus this wait, so
  `loop { play().await }` after a call ends is a bare busy loop that no `abort()` can take back;
  the hop is what returns the runtime worker, and by then there are no clip ends left to order.
  Every other end happened once to one clip and takes no hop, which is what keeps two watchers
  waking in the order their clips ended.
  Verified on `main` rather than on the branch: `sipx-call`'s `playback` suite 7 of 7 including
  `every_playback_reports_its_own_end_by_id`, `sipx-media`'s `play_cancellation` 3 of 3, and both
  crates' full suites green with clippy clean. The branch's own worktree could not run the
  `sipx-call` half — it failed to compile `sipx-media`'s lib against a `sipx-rtp` whose report
  structs are non-exhaustive on `main` and not on that branch, which is a property of a scratch
  checkout mid-wave and not of this change.

- 2026-08-09: closed at the `1.0.0-rc.13` boundary, against the wave gate run on this tree.
