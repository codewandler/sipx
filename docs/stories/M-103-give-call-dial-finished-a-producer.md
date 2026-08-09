---
id: M-103
title: Give `call.dial.finished` a producer
pillar: Media
status: done
priority: 29
design: docs/designs/app-sdk.md
epic: app-sdk
areas: [sipx-app, sipx-app-protocol, app-sdk, m16]
predicate:
announcement:
note: filed by M-99 · the last §5.3 row with no producer anywhere, and the driver refuses the effect that would create the leg
---

# Give `call.dial.finished` a producer

## Goal

Close the third and last instance of the defect `M-98` and `M-99` closed twice: a §5.3 row that is
specified, typed, wire-round-tripped, consumed — and emitted by nothing.

## Acceptance

- [x] A failing-first test proves an app-protocol client is told how its `dial` resolved. Nothing
      produces the row today: `grep -rn 'EventKind::DialFinished' crates/` finds only *consumers*
      (`interpreter.rs:486` matches it against the running instruction, `interpreter.rs:1014`
      updates the snapshot's `legs`) plus the §5.3 fixture and scripted harness data. No `src` arm
      anywhere constructs one.
- [x] The driver can perform `Effect::Dial`. `crates/sipx-app/src/host.rs` has no arm for it, so it
      falls into `_ => self.fail_effect()` with `Effect::Bridge`, `Effect::Unbridge`,
      `Effect::Record` and the transfers — the phase-1 limit its own comment states. A row whose
      effect is refused cannot be reached by any path, which is why this is one story and not two.
- [x] `tests/spec_tables.rs`'s `COMPOSED_BY_THE_DRIVER` is empty, or its last entry has a producer a
      test names. `M-99` left `call.incoming` (`host.rs:261`) and `call.gather.finished`
      (`interpreter.rs:550`) in that list with real producers behind them and `call.dial.finished`
      in it with none — the list asserts a row is *absent* from `src/call.rs` and has never asserted
      that anything composes it, which is the hole all three of these stories fell into.
- [x] The full gate is green.

## Progress

- Backlog. Filed by `M-99` on 2026-08-09, which gave `call.bridged` and `call.unbridged` an arm in
  `event_from_call` and found this row next to them, in the same list, with the same absence.

### 2026-08-09 — the row has a producer, and the list now proves one

**Where the producer went, and why it is not in the bridge.** `crates/sipx-app/src/host.rs`
composes `EventKind::DialFinished` in `apply_action`, from an `ActorAction::Dialed` its own
`Effect::Dial` arm produced. It could not have gone into `event_from_call`, and the reason is
sharper than "a different leg": **`sipx-call` never puts a dial's outcome on an event at all.**
`Dialing::confirm` returns `Err(Error::Rejected { status, .. })` and drops its event sink without an
ending, so a driver watching the outbound leg's `CallEvents` would see the stream close and never
learn that the status was 486. The vocabulary half of the mapping is still in the one module that
owns it — `sipx_app_protocol::dial_outcome` turns a `sipx_call::Error` into §5.3's four words — and
only the composing, which needs the app's `instruction_id` and `leg` as well, is the driver's.

**How `COMPOSED_BY_THE_DRIVER` changed.** It was `[&str; 3]`, and the only thing asserted about a
name on it was that `src/call.rs` does *not* produce it. That is satisfied by a row nothing produces
anywhere, which is why `M-98`, `M-99` and this story all had to be filed separately. Each entry is
now `(row, path, source)` and carries a second assertion: the named file must contain the
`EventKind::` construction. `call.incoming` and `call.gather.finished` were checked against their
real producers for the first time by that change and both hold. No other row was hiding in the
list — every §5.3 row outside it already had a bridge arm, which the existing branch was checking.

**What the driver does with an answered leg.** `Effect::Bridge` is still refused, so a leg that
answers has nothing to be coupled to. It is served by `sipx_call::serve_until` on its own task —
answering its own BYE and session refreshes — and `DialTasks::finish` stops and joins every leg
before the actor reports its own end, so N11's accounting stays true. A future `bridge` story needs
a way to reach that `Call` from the actor; today it deliberately has none.

**Owed CHANGELOG sentence** (the coordinator writes it): *A host can now perform the contract's
`dial` verb: an outbound leg is placed on the call's own endpoint and §5.3's `call.dial.finished`
reports how it resolved, which no host could emit before.*

**Left open**, filed as `M-108`: an answered `dial` leg that the far end later hangs up has no §5.3
event. `call.unbridged` reports a coupling ending and requires one to have been made; §5.2's `legs`
is only rewritten by `call.dial.finished`. So an app holding a leg it has not bridged is never told
it went away.

## Notes

- The coupling half is done and this is what it left behind. `M-99`'s producer takes the far leg's
  name from the caller (`event_from_call`'s `bridged_leg`), because the app named it in its `bridge`
  instruction — and the interpreter mints that name in `name_a_leg` when it issues `Effect::Dial`.
  So the leg vocabulary already exists end to end; what does not exist is anything that places the
  second call.
- `sipx-app`'s host passes `None` for `bridged_leg` today and says why in a comment beside the call.
  When it can perform `Effect::Bridge` it must pass the `leg` the interpreter put on that effect,
  and hold the name until the `CallEvent::Unbridged` it produces has been mapped — §6.2 ends a
  bridge on `unbridge` **or either leg ending**, so the name outlives the instruction.

- 2026-08-09: closed at the `1.0.0-rc.17` boundary, against the wave gate run on this tree.
