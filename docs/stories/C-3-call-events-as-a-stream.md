---
id: C-3
title: Report call state as a typed event stream
pillar: Signalling
status: done
priority:
design: docs/designs/app-sdk.md
epic: app-sdk
areas: [sipx-call]
note: app-sdk keystone · the other stories report through this · size M
---

# Report call state as a typed event stream

## Goal
A `Call` reports what happens to it — ringing, answered, a digit, playback finished, a transfer
requested, ended and why — as a typed, channel-backed event stream a host can consume, instead of
state that is only visible by calling methods on the `Call` at the right moment.

## Acceptance
- [x] A `CallEvent` enum covering at least: ringing (with reliability), answered, DTMF digit
      received, playback finished, recording finished, transfer requested (inbound REFER, carrying
      the target), transfer progress (`TransferState` transitions), hold and resume by the far end,
      and ended with a cause that distinguishes local hangup, remote BYE, rejection (status), and
      timeout. The variants carry what
      [`docs/specs/app-contract.md`](../specs/app-contract.md) needs to build its per-event call
      snapshot.
- [x] Each `Call` exposes an owned event receiver — channel-backed, one consumer, per the vision's
      principle 3. A slow consumer must not stall the call's signalling; the overflow behaviour is
      decided, documented, and tested (bounded queue with a defined policy, never an unbounded
      buffer, never a silent drop of `ended`).
- [x] `Call::handle` and `serve()` emit through the same path — an event is emitted where the state
      changes, not reconstructed after the fact, so the stream cannot disagree with the call.
- [x] Events are push, not poll: no clock reads added to `sipx-call`; timer-driven transitions
      (session expiry) emit when the timer input fires.
- [x] Failing-first test: `hanging_up_emits_ended_with_cause`.

## Progress
- Done. `CallEvent` and `EndCause` in `crates/sipx-call/src/event.rs`, a `CallEvents` receiver
  handed out exactly once, an internal `EventSink`, emission wired through `call.rs`, and ten
  integration tests in `crates/sipx-call/tests/events.rs`.
- **The overflow policy is the part worth knowing.** A bounded channel of `CAPACITY = 32` with
  **one slot reserved permanently for `Ended`** at construction, before any ordinary event can
  claim it. Ordinary events compete for the other 31 and are dropped rather than queued when the
  consumer is behind; `Ended` spends the reservation and so can never be the one an overflow
  drops. Nothing on the ending path awaits the channel having room, which is what makes a
  consumer that never reads unable to stall a call's teardown — tested by taking the stream,
  never reading it, and hanging up.
- Dropping rather than blocking is the right trade here because the contract's events each carry
  a snapshot: a consumer that missed one resynchronises from the next. That is not true of
  `Ended`, which is why it is the one event given a guarantee rather than a policy.
- **Playback and recording now have producers.** `Call::play` and `Call::record_until_idle`
  emit `PlaybackFinished { completed }` and `RecordingFinished { duration }`. This was the gap
  the first pass left open, and closing it needed only what `sipx-media` already had —
  `MediaSession::play` and `record_until_idle` both resolve — plus two things:
  - `MediaSession::play` now returns whether the clip reached the end instead of `()`. "The
    announcement finished" and "the caller hung up during it" are different things to whatever
    decides what happens next, and returning `()` made them indistinguishable.
  - `MediaSession::samples_per_packet()` is exposed, so `Call::play` uses the session's own
    packet size rather than a caller's literal `160` — which is wrong for any codec whose clock
    is not 8 kHz.
  - The recorded duration is measured from the samples and the negotiated clock rate, **not**
    from how long this side waited. Counting the idle timeout would describe our own patience
    rather than the recording, and would change if anyone tuned the timeout.
- **`EndCause::Rejected` has no producer at this layer, and that is the finished answer rather
  than a leftover.** A `Call` does not exist until an INVITE has already succeeded (2xx and
  ACK), so by the time there is a call to end, refusing it is no longer possible — what ends an
  answered call is a BYE. Refusing happens before a `Call` is built. The variant stays in the
  enum because the app-visible call of `C-4`/`A-2` exists from the incoming INVITE onward and
  will produce it, and because adding a variant after the fact is the wire-breaking change §4 of
  the contract spec warns about.
  - The first pass documented this variant as "the status the far end gave", which is the wrong
    direction: the contract's `rejected{status}` on `call.ended` is *this side* refusing an
    invitation (§5.3 `reject`). The far end refusing an attempt of ours is `call.dial.finished`
    with outcome `rejected` — a different event about a different leg. Corrected.
- Mutation-tested: reporting a cut-short playback as completed, measuring the recording as the
  idle timeout, and not emitting `PlaybackFinished` at all — each fails a test.
- No clock reads were added to `sipx-call`; the session-timer transition emits when the timer
  input fires, and `dial` and `serve` go through the same emission path so the stream cannot
  disagree with the call.
- Fixed on the way: two public doc comments linked to the crate-private `EventSink`, which fails
  `./scripts/build-docs.sh` at `-D warnings`.

## Notes
- The keystone of the `app-sdk` epic: `C-4` dispatches by consuming these events, `M-17`'s
  playback completion and `M-18`'s gate report through them, and `C-5`'s interpreter consumes
  them as its input alphabet.
- Needed by the host (`crates/sipx-app`, story `A-2`); the sans-IO interpreter (`C-5`) is
  unusable without an event source.
- Today the only inbound "event" is `sipx_transport::Incoming` plus transaction-level `TuEvent` —
  nothing at the call layer. `crates/sipx-call/tests/call.rs` works around it with
  `Arc<Mutex<Call>>`, which is exactly the shape the vision forbids.
