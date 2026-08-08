---
id: A-40
title: Drive speech sessions asynchronously
pillar: Application
status: ready
priority: 6
design: docs/designs/local-speech.md
epic: local-speech
areas: [sipx-media, speech, m16]
predicate:
announcement:
note: A-39 built the contract, not the shell that runs it · owns §10's three driver-side vectors, which cannot run until it exists
---

# Drive speech sessions asynchronously

## Goal

Build §2's asynchronous driver: the shell that pumps a selected provider's recognition and synthesis
sessions off the media seam, bounds their unconsumed output, and honours a drain deadline. `A-39`
built the contract and proved it executable; nothing yet runs it.

## Acceptance

- [ ] A driver consumes `A-39`'s selection result, attaches through `M-54`'s seam and runs a
      recognition and a synthesis session to completion without blocking RTP decode, encode,
      playback or capture.
- [ ] **REC-7 runs**: unconsumed output is bounded by `SpeechBounds::unconsumed_outputs` and
      coalesces at the bound rather than growing, proved by a failing-first vector.
- [ ] **LIF-6 runs**: the drain deadline (`DeadlineKind::Drain`) expiring yields
      `Stopped { aborted: true }`, and a clean drain does not.
- [ ] Cancellation and call teardown drop and join every driver task within the stated bound; no
      fixed sleep substitutes for an observed barrier.
- [ ] Still no speech implementation, model, accelerator dependency or audio retention ships — the
      inert provider remains the only one in the tree.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `A-39`'s handoff. Three of §10's 34 conformance vectors are structurally
  unrunnable without this driver: REC-7 (the unconsumed-output bound and its coalescing) and LIF-6
  (the drain deadline's aborted `Stopped`). REC-3, the third of that family, *does* run in `A-39`,
  because the queue it bounds is `M-54`'s seam and that exists. The values a driver needs are
  already public — `SpeechBounds::unconsumed_outputs`, `DeadlineKind::Drain`, `Stopped { aborted }`.

## Notes

- Sequence: this should land **before** `M-55`/`M-56`. A real provider written against an undriven
  contract will encode the driver's absence into its own shape.
- `A-28` (isolation and retention) is independent of this and now has types to run against.
- Two `A-39` risks this story inherits: `Selected::processing` passes `SpeechBounds::input_frames`
  straight to the seam's `queue_capacity`, so an out-of-domain bound surfaces as the seam's
  `ProcessingError::QueueCapacity` at attach time rather than a `SpeechBounds` error at configure
  time — the two domains are compatible today but are not the same domain.
