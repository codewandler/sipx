---
id: M-45
title: Hold incoming audio in a jitter buffer
pillar: Media
status: done
priority:
design: docs/designs/demand.md
epic: demand
areas: [sipx-rtp, sipx-media]
predicate:
announcement:
note: two independent field reports of seconds of added delay · characterise the current buffer before changing it
---

# Hold incoming audio in a jitter buffer

## Goal

Make incoming audio play out smoothly under network jitter, loss and reordering, with a buffer whose
depth is bounded and observable rather than growing into latency.

## Acceptance

- [x] **Characterise the current behaviour first.** `sipx-rtp` has a jitter buffer; Progress records
      what it does today under jitter, loss, reordering and duplication, measured, before any change.
      A story that changes it without that baseline cannot show an improvement.
- [x] A failing-first test drives a synthetic packet stream with controlled jitter and reordering
      through the buffer and asserts the play-out sequence, using virtual time — **not** a wall-clock
      sleep, per `scripts/check-fixed-sleep.py` and `docs/designs/media.md`.
- [x] Buffer depth is bounded and does not grow monotonically under sustained jitter. The reported
      field failure is seconds of accumulated delay, so the test asserts a bound on added latency,
      not merely that audio is continuous.
- [x] Late and duplicate packets are discarded rather than played, and sequence wrap is handled.
- [x] Loss is concealed by a stated, documented strategy — silence insertion is acceptable if it is
      written down; an undocumented gap that produces a click is not.
- [x] Buffer depth, discards and concealment events are visible through the existing counters, per
      the rule that shedding must never be invisible.
- [x] `./scripts/gate.py` green, including `check-fixed-sleep.py`.

## Progress
- (not started)

- 2026-08-08: **readiness audit — ready.** Two clarifications: the `media-runtime.md` §4 counter
  specification edit is in scope, and closing this story as "characterised, no code change needed"
  is an acceptable outcome if the measurement says so — the story asks for the buffer to be bounded
  and observable, not for it to be rewritten.

- 2026-08-08: **characterised, and the buffer was not the defect.** Measured before changing
  anything, on the drain the media session actually uses — push on arrival, then pop until the
  buffer refuses — rather than on `jitter_traces.rs`'s fixed playout clock. That distinction is the
  whole measurement: on the arrival-driven drain the time a packet spends held *is* the latency the
  buffer charges, and a metronome consumer hides it.

  1500 G.711 packets at 20 ms, `adaptive(3, 12)`, hold time in milliseconds:

  | trace | hold max | hold mean | final depth | peak depth | late | lost | dup |
  |---|---|---|---|---|---|---|---|
  | constant 5 ms delay | 100 | 40.1 | 3 | 3 | 0 | 0 | 0 |
  | constant 5 ms, `new(3)` fixed | 100 | 40.1 | 3 | 3 | 0 | 0 | 0 |
  | jitter, 5–65 ms uniform | 114 | 40.1 | 3 | 3 | 0 | 0 | 0 |
  | 300 ms spike on every third packet | 515 | 200.8 | 11 | 11 | 13 | 12 | 0 |
  | 9 % loss | 100 | 44.1 | 3 | 3 | 0 | 136 | 0 |
  | every other pair swapped on the wire | 85 | 40.1 | 3 | 3 | 0 | 0 | 0 |
  | 20 % duplicated | 100 | 39.5 | 3 | 3 | 0 | 0 | 300 |
  | one packet 3 s late | 100 | 60.9 | 3 | 6 | 1 | 1 | 0 |
  | 1 s stall then 50 packets at once | 140 | 85.5 | 3 | 8 | 0 | 0 | 0 |
  | 600 packets across the 16-bit wrap, under jitter | — | — | 3 | 3 | 0 | 0 | 0 |

  **The buffer does not grow into latency.** Depth never passed its ceiling, packets held never
  exceeded depth, and the depth came back to its floor of 3 after both the straggler and the stall.
  The worst hold in any trace was 515 ms, on a network delivering a third of its packets 300 ms
  late — bad enough that nobody would stay on the call. Play-out order was strictly ascending in
  every trace, including across the sequence wrap; late and duplicate packets were refused. So the
  two field reports of *seconds* of delay are not this buffer, and rewriting it would have been
  motion rather than a fix.

  Two real defects fell out of the measurement, and both are fixed here.

  1. **Loss was not concealed.** The buffer counted a gap and played straight over it, so the two
     packets either side of a lost one were handed to the application back to back: a step in the
     waveform at the splice, and a permanent 20 ms of drift per lost packet. Nine per cent loss
     removed 136 packets' worth of timeline from a 30-second trace. The receive loop now fills each
     missing slot with one packetisation interval of silence, bounded at 200 ms of consecutive loss
     — in time, not in packets, because a packet is 10 ms in one codec and 60 in another — because
     a longer run is a partitioned far end and filling it would inject the whole outage as silence.
     Documented in `media-runtime.md` §4.2.
  2. **The buffer's discards were invisible.** `receive_loop` dropped `push_at`'s answer on the
     floor, so a call losing audio to a too-shallow buffer looked exactly like a healthy one —
     the invisible shedding §4 exists to prevent. `push_at` is now `#[must_use]`, and the refusals
     land in `MediaDiscardCounts` as `jitter_late` and `jitter_duplicates`, with concealed slots as
     `jitter_concealed`.

  Not changed, deliberately: depth stays counted in packets. It is a real sharp edge — the same
  wall-clock jitter cost a mean hold of 40 ms at a 20 ms packet time and 119 ms at 60 ms — but it
  is a *sizing* inconsistency, not the unbounded growth this story was filed about, and converting
  the unit is a behaviour change on every call with no measurement here demanding it.

  **Where the seconds probably are, and it is not this story.** `MediaSession`'s inbound audio
  channel is `mpsc::channel::<Vec<i16>>(256)` (`session.rs:1574`), one frame per packet, delivered
  with `send().await`. At 20 ms a frame that is **5.12 seconds** of audio that can queue behind the
  jitter buffer before backpressure reaches the socket, with no bound in time and no counter. An
  application reading even slightly slower than real time settles at the far end of it. That
  matches both field reports far better than a buffer whose worst measured hold is half a second.
  Filed as an adjacent finding rather than fixed: changing that queue's depth or policy changes the
  delivery contract for every application, which deserves its own story and its own measurement.

- 2026-08-08: closed against a green gate — `./scripts/gate.py` reported **40 steps, all green** on `main` at `1256b8e`. An earlier run on the same tree failed two `sipx-cli` audio tests; both pass in isolation and in the full 83-test `cli.rs` binary, and `M-59` independently hit the same class on a sibling test while five other checkouts were building on this shared box. `X-118` owns that flakiness.

## Notes
- Two independent field reports in the demand survey, one comparing unfavourably against a desktop
  softphone. It is the difference between "audio works" and "audio sounds right", and it is the kind
  of defect that never appears on a LAN.
- Related but distinct from `M-43`: this is play-out timing, that is format conversion.
