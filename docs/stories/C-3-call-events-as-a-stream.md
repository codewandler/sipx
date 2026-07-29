---
id: C-3
title: Report call state as a typed event stream
pillar: Signalling
status: in-progress
priority: 4
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
- [ ] A `CallEvent` enum covering at least: ringing (with reliability), answered, DTMF digit
      received, playback finished, recording finished, transfer requested (inbound REFER, carrying
      the target), transfer progress (`TransferState` transitions), hold and resume by the far end,
      and ended with a cause that distinguishes local hangup, remote BYE, rejection (status), and
      timeout. The variants carry what
      [`docs/specs/app-contract.md`](../specs/app-contract.md) needs to build its per-event call
      snapshot.
- [ ] Each `Call` exposes an owned event receiver — channel-backed, one consumer, per the vision's
      principle 3. A slow consumer must not stall the call's signalling; the overflow behaviour is
      decided, documented, and tested (bounded queue with a defined policy, never an unbounded
      buffer, never a silent drop of `ended`).
- [ ] `Call::handle` and `serve()` emit through the same path — an event is emitted where the state
      changes, not reconstructed after the fact, so the stream cannot disagree with the call.
- [ ] Events are push, not poll: no clock reads added to `sipx-call`; timer-driven transitions
      (session expiry) emit when the timer input fires.
- [ ] Failing-first test: `hanging_up_emits_ended_with_cause`.

## Progress
- 2026-07-29: an implementation pass was started by an agent and interrupted before it ran the
  gate. **That work is now committed and the whole gate is green on it** — 951 tests, clippy
  clean at `-D warnings`, every feature combination building, docs building. The T-18 blockage
  the earlier note mentioned is gone: T-18 shipped in 0.4.0.
- Committed as **work in progress, not done.** What exists: `CallEvent` and `EndCause` in
  `crates/sipx-call/src/event.rs`, a `CallEvents` receiver handed out exactly once, an internal
  `EventSink`, emission wired through `call.rs`, and seven integration tests in
  `crates/sipx-call/tests/events.rs` including the story's `hanging_up_emits_ended_with_cause`
  and an overflow test (`a_consumer_that_never_reads_does_not_stall_hanging_up`).
- The design decisions the earlier note said were unrecorded are now readable in the module
  docs, which is where they belong: a bounded channel of `CAPACITY = 32` with **one slot
  reserved permanently for `Ended`**, so an overflow can never drop the one event that says the
  call is over, and a consumer that stops reading cannot stall the call's signalling.
- **Two variants have no producer, both deliberately**, and this is what is left to settle
  before the story closes:
  - `PlaybackFinished` and `RecordingFinished` — nothing in `sipx-call` can start a playback or
    recording that resolves; that machinery is `M-17`'s. The variants fix the vocabulary's shape
    ahead of it.
  - `EndCause::Rejected` — structural rather than sequencing. A `Call` does not exist until an
    INVITE has succeeded (2xx and ACK), so nothing at this layer can reject *this* call's own
    attempt; a rejection ends an invitation before a `Call` is built. Kept in the enum because
    adding a variant later is the wire-breaking change §4 of the contract spec warns about.
- **Decide before closing:** whether acceptance is met by variants that exist but do not yet
  emit, or whether `C-3` waits on `M-17` and the `C-4`/app-host call-lifecycle change. That is a
  scope call, not a coding one — hence still `in-progress`.
- Fixed on the way in: two public doc comments linked to the crate-private `EventSink`, which
  fails `./scripts/build-docs.sh` at `-D warnings`. They now describe the policy instead of
  linking to the internal type.

## Notes
- The keystone of the `app-sdk` epic: `C-4` dispatches by consuming these events, `M-17`'s
  playback completion and `M-18`'s gate report through them, and `C-5`'s interpreter consumes
  them as its input alphabet.
- Needed by the host (`crates/sipx-app`, story `A-2`); the sans-IO interpreter (`C-5`) is
  unusable without an event source.
- Today the only inbound "event" is `sipx_transport::Incoming` plus transaction-level `TuEvent` —
  nothing at the call layer. `crates/sipx-call/tests/call.rs` works around it with
  `Arc<Mutex<Call>>`, which is exactly the shape the vision forbids.
