---
id: A-40
title: Drive speech sessions asynchronously
pillar: Application
status: in-progress
priority: 11
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

- [x] A driver consumes `A-39`'s selection result, attaches through `M-54`'s seam and runs a
      recognition and a synthesis session to completion without blocking RTP decode, encode,
      playback or capture.
- [x] **REC-7 runs**: unconsumed output is bounded by `SpeechBounds::unconsumed_outputs` and
      coalesces at the bound rather than growing, proved by a failing-first vector.
- [x] **LIF-6 runs**: the drain deadline (`DeadlineKind::Drain`) expiring yields
      `Stopped { aborted: true }`, and a clean drain does not.
- [x] Cancellation and call teardown drop and join every driver task within the stated bound; no
      fixed sleep substitutes for an observed barrier.
- [x] Still no speech implementation, model, accelerator dependency or audio retention ships — the
      inert provider remains the only one in the tree.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `A-39`'s handoff. Three of §10's 34 conformance vectors are structurally
  unrunnable without this driver: REC-7 (the unconsumed-output bound and its coalescing) and LIF-6
  (the drain deadline's aborted `Stopped`). REC-3, the third of that family, *does* run in `A-39`,
  because the queue it bounds is `M-54`'s seam and that exists. The values a driver needs are
  already public — `SpeechBounds::unconsumed_outputs`, `DeadlineKind::Drain`, `Stopped { aborted }`.
- 2026-08-08: driver built in `crates/sipx-media/src/speech/driver.rs` — `RecognitionDriver`,
  `SynthesisDriver`, `DriverError` — with six vectors in `crates/sipx-media/tests/speech.rs`. Both
  of §10's stranded vectors now run, and the two REC-7 halves are deliberately separate tests:
  coalescing needs a seam deep enough to lose nothing, and the input degradation needs one shallow
  enough to lose almost everything, so one test cannot assert both exactly. The saturation point is
  the fixture's own arithmetic rather than a race — `Warming` and `Ready` are lifecycle outputs, so
  a bound of two is reached before the first frame is consumed and the driver provably consumes
  none — which is what makes `10 × 160` the exact sample time of the surviving audio.
- 2026-08-08: three normative gaps in `speech-providers.md` were closed rather than guessed at,
  because each admitted two implementations: §2 now says the deadline *generation* is the driver's
  and that the driver is what discards a stale firing (a provider is told nothing about arming, so
  it could not have been the provider's job); §5 fixes the order of the drain abort — deliver
  `DeadlineFired(Drain)`, apply what it produces, abort only then — and states that the output bound
  pauses consumption without ever delaying the driver's own terminal; §6 says window credit returns
  when a chunk is *handed on*, since returning it on receipt would move the bounded memory one queue
  along rather than bound it.
- 2026-08-08: `./scripts/gate.py` deliberately not run here — the wave coordinator runs one gate
  over the merged tree. Focused verification ran green: `cargo test -p sipx-media --all-features`
  (13 targets, 302 tests), `cargo clippy -p sipx-media --all-targets --all-features --no-deps`,
  `cargo fmt --check`, `check-fixed-sleep.py --check`, `check-provenance.sh`, `check-docs-links.py`,
  `check-app-surface.py --check`, `check-audio-claims.py --check`, and a docs build with
  `RUSTDOCFLAGS=-D warnings`.

## Notes

- Sequence: this should land **before** `M-55`/`M-56`. A real provider written against an undriven
  contract will encode the driver's absence into its own shape.
- `A-28` (isolation and retention) is independent of this and now has types to run against.
- Two `A-39` risks this story inherits: `Selected::processing` passes `SpeechBounds::input_frames`
  straight to the seam's `queue_capacity`, so an out-of-domain bound surfaces as the seam's
  `ProcessingError::QueueCapacity` at attach time rather than a `SpeechBounds` error at configure
  time — the two domains are compatible today but are not the same domain.
