---
id: M-103
title: Give `call.dial.finished` a producer
pillar: Media
status: backlog
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

- [ ] A failing-first test proves an app-protocol client is told how its `dial` resolved. Nothing
      produces the row today: `grep -rn 'EventKind::DialFinished' crates/` finds only *consumers*
      (`interpreter.rs:486` matches it against the running instruction, `interpreter.rs:1014`
      updates the snapshot's `legs`) plus the §5.3 fixture and scripted harness data. No `src` arm
      anywhere constructs one.
- [ ] The driver can perform `Effect::Dial`. `crates/sipx-app/src/host.rs` has no arm for it, so it
      falls into `_ => self.fail_effect()` with `Effect::Bridge`, `Effect::Unbridge`,
      `Effect::Record` and the transfers — the phase-1 limit its own comment states. A row whose
      effect is refused cannot be reached by any path, which is why this is one story and not two.
- [ ] `tests/spec_tables.rs`'s `COMPOSED_BY_THE_DRIVER` is empty, or its last entry has a producer a
      test names. `M-99` left `call.incoming` (`host.rs:261`) and `call.gather.finished`
      (`interpreter.rs:550`) in that list with real producers behind them and `call.dial.finished`
      in it with none — the list asserts a row is *absent* from `src/call.rs` and has never asserted
      that anything composes it, which is the hole all three of these stories fell into.
- [ ] The full gate is green.

## Progress

- Backlog. Filed by `M-99` on 2026-08-09, which gave `call.bridged` and `call.unbridged` an arm in
  `event_from_call` and found this row next to them, in the same list, with the same absence.

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
