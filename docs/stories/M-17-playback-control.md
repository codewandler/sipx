---
id: M-17
title: Control playback — queue, stop, interrupt on digit
pillar: Media
status: done
priority:
design: docs/designs/app-sdk.md
epic: app-sdk
areas: [sipx-media, sipx-call]
note: app-sdk · after C-3 · gates the contract's gather · size S/M
---

# Control playback — queue, stop, interrupt on digit

## Goal
Playback that can be queued, stopped, and interrupted by a DTMF digit — the primitive under
"play a prompt and collect digits", which no IVR-shaped application can be built without.

## Acceptance
- [x] Starting playback returns a handle; the handle can stop playback, and stopping takes effect
      within a bounded number of packet intervals (bound stated and tested).
- [x] Playback can be started with interrupt-on-digit: a received DTMF event (RFC 4733) halts
      playback within the same bound, and the interrupting digit is not lost — it is delivered to
      the digit consumer.
- [x] Clips queue: starting a second playback while one is active either queues it or replaces the
      queue — the choice is the design's to make, recorded in `docs/designs/app-sdk.md`, and both
      the chosen behaviour and its edge (queue while stopping) are tested.
- [x] Completion and interruption are observable as events (`C-3`), each carrying which playback
      finished and whether it ran to completion or was cut.
- [x] Failing-first test: `a_digit_interrupts_playback`.

## Progress
- Implemented on `impl/M-17`. All five acceptance items satisfied; see
  `crates/sipx-call/tests/playback.rs` (7 tests) and the new unit tests in
  `crates/sipx-media/src/session.rs`.
- **Queue-vs-replace decided: clips queue.** Recorded in `docs/designs/app-sdk.md` together with
  the queue-while-stopping edge, the bound, and why discarding a stopped clip's frames is not the
  case `M-18`'s RFC 3550 §6 rule forbids.
- **The bound is `Playback::STOP_BOUND_PACKETS` = 2 packet intervals**, for `stop` and for
  interrupt-on-digit alike. Measured against what actually reached the wire, cross-checked against
  the far end's receive count. Disabling the drain makes both bound tests fail at ~30 packets.
- Shape: `MediaSession::start_playback(samples, Interrupt) -> Playback`, `Playback::{id, stop,
  is_stopped, end, finished}`, `PlaybackEnd::{Completed, Stopped, Interrupted, SessionEnded,
  Refused}`; mirrored on `Call::start_playback`. `Call::play` is that awaited with
  `Interrupt::Never` and keeps its signature.
- Interruption is armed in the *receive* path (a `watch` counter bumped only when the digit was
  accepted onto the application's channel), so the interrupting digit is never consumed.
- `Playback` has two waits: `play_out` (stops the clip if the wait is dropped) and `finished`
  (observes only). `play` is the former, so `timeout(d, play(..))` — what `sipx-cli answer
  --play --duration` does — keeps stopping the audio rather than returning while it plays on.
- `CallEvent::PlaybackFinished` gained a `playback: PlaybackId` field. Its only consumers were
  inside this crate; `tests/events.rs` patterns were widened with `..`.
- Not done here (fenced): CHANGELOG, board, roadmap. `sipx-cli/src/answer.rs` still wraps
  `media.play` in a `timeout` — the workaround this story removes the need for; left for a
  follow-up since `sipx-cli` is outside this story's scope.

## Notes
- Today `MediaSession::play(&samples, spp)` runs a clip to its end with no handle, no stop, and no
  interruption (`crates/sipx-media/src/session.rs`); `collect_digits` exists but cannot be
  combined with a prompt except by racing two futures by hand — and the prompt keeps playing.
- Needed by the host (`crates/sipx-app`, story `A-2`): the contract's
  `gather{prompt, interruptible}` instruction is unimplementable without this.
