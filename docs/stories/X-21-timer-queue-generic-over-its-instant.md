---
id: X-21
title: Make the timer queue generic over its instant
pillar: Build
status: done
priority:
design:
epic:
areas: [sipx-transport]
note: the queue documents clock-independence its signature contradicts · additive, breaks nothing
---

# Make the timer queue generic over its instant

## Goal
`TimerQueue` is generic over its key but hardcoded to `tokio::time::Instant`, so the one caller it
was generalised *for* — a driver on virtual time — still cannot use it. Make the instant a type
parameter defaulting to `tokio::time::Instant`, so the module's existing claim about not having an
opinion on what an instant means becomes true.

## Acceptance
- [x] `TimerQueue<K, I = Instant>` with `I: Ord + Copy + Add<Duration, Output = I>`; `set`,
      `take_due` and `next_deadline` speak `I`.
- [x] Every existing caller compiles **unchanged** — `endpoint.rs`'s
      `TimerQueue<(TransactionKey, Timer)>` keeps its meaning through the default type parameter.
- [x] Failing-first test: `a_virtual_clock_drives_the_queue_with_no_runtime`, using a
      `Virtual(u64)` instant, no tokio runtime and no clock read anywhere.
- [x] The module documentation says what the parameter is for, since the doc's claim about
      clock-independence is what this story exists to make true.

## Progress
- Done. `TimerQueue<K, I = Instant>` with `I: Ord + Copy + Add<Duration, Output = I>`. The bound is
  the minimum the queue actually uses: compare deadlines, copy them out of entries, and add a
  `Duration` to get one. Nothing about earliest-deadline-first needs more than that.
- **The default type parameter is what makes this additive.** `TimerQueue<K>` still names the
  `tokio::time::Instant` queue, so `endpoint.rs`'s `TimerQueue<(TransactionKey, Timer)>` compiles
  untouched — asserted by a test that names the queue with one parameter and hands it a real
  `Instant`, rather than left to the build to notice.
- The `Ord`/`PartialOrd` bounds moved from the key to the instant, because ordering is by deadline
  alone: two entries with the same deadline are interchangeable to this queue.
- Failing-first test `a_virtual_clock_drives_the_queue_with_no_runtime` uses a `Virtual(u64)` tick
  counter and is deliberately **not** a `#[tokio::test]` — no runtime, no clock. It failed to
  compile before the change for exactly the reason the story describes: the queue took one type
  parameter and wanted an `Instant`.
- Cleared on the way in: clippy's `cast_possible_truncation` on the virtual clock's millisecond
  arithmetic (now a saturating `try_from`) and `duration_suboptimal_units`.

## Notes
- `crates/sipx-transport/src/timers.rs`. The module doc already claims "nothing here has an opinion
  about what an instant means", which is false while the field is a `tokio::time::Instant`:
  that type's only constructors are `now()`, which reads the machine clock, and `from_std`, which
  needs a `std::time::Instant` that has no zero either. A simulator on virtual time has no instant
  to hand in and cannot build one.
- `X-14` generalised the queue over its key and moved the clock out of it. This is the half that
  story left: the *reading* of the clock moved to the caller, but the *type* did not.
- Additive by construction. A default type parameter means `TimerQueue<K>` still names what it
  always named, so this is not a breaking change to `sipx-transport`.
