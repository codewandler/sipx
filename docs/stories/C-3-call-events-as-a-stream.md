---
id: C-3
title: Report call state as a typed event stream
pillar: Signalling
status: backlog
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
- Not started.

## Notes
- The keystone of the `app-sdk` epic: `C-4` dispatches by consuming these events, `M-17`'s
  playback completion and `M-18`'s gate report through them, and `C-5`'s interpreter consumes
  them as its input alphabet.
- Needed by the host (`crates/sipx-app`, story `A-2`); the sans-IO interpreter (`C-5`) is
  unusable without an event source.
- Today the only inbound "event" is `sipx_transport::Incoming` plus transaction-level `TuEvent` —
  nothing at the call layer. `crates/sipx-call/tests/call.rs` works around it with
  `Arc<Mutex<Call>>`, which is exactly the shape the vision forbids.
