---
id: M-17
title: Control playback — queue, stop, interrupt on digit
pillar: Media
status: backlog
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
- [ ] Starting playback returns a handle; the handle can stop playback, and stopping takes effect
      within a bounded number of packet intervals (bound stated and tested).
- [ ] Playback can be started with interrupt-on-digit: a received DTMF event (RFC 4733) halts
      playback within the same bound, and the interrupting digit is not lost — it is delivered to
      the digit consumer.
- [ ] Clips queue: starting a second playback while one is active either queues it or replaces the
      queue — the choice is the design's to make, recorded in `docs/designs/app-sdk.md`, and both
      the chosen behaviour and its edge (queue while stopping) are tested.
- [ ] Completion and interruption are observable as events (`C-3`), each carrying which playback
      finished and whether it ran to completion or was cut.
- [ ] Failing-first test: `a_digit_interrupts_playback`.

## Progress
- Not started.

## Notes
- Today `MediaSession::play(&samples, spp)` runs a clip to its end with no handle, no stop, and no
  interruption (`crates/sipx-media/src/session.rs`); `collect_digits` exists but cannot be
  combined with a prompt except by racing two futures by hand — and the prompt keeps playing.
- Needed by the host (`crates/sipx-app`, story `A-2`): the contract's
  `gather{prompt, interruptible}` instruction is unimplementable without this.
