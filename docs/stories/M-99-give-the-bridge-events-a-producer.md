---
id: M-99
title: Give `call.bridged` and `call.unbridged` a producer
pillar: Media
status: in-progress
priority: 28
design: docs/designs/app-sdk.md
epic: app-sdk
areas: [sipx-call, sipx-app, sipx-app-protocol, app-sdk, m16]
predicate:
announcement:
note: filed by M-98 · two more §5.3 rows are specified, typed and round-tripped with nothing that emits them
---

# Give `call.bridged` and `call.unbridged` a producer

## Goal

Make §5.3's `call.bridged` and `call.unbridged` reachable, or say in the specification that they
are not — the same question `M-98` answered for `call.signal.metrics`, one layer up.

## Acceptance

- [x] A failing-first test proves an app-protocol client is told when the media coupling on its
      call changed. Nothing produces either row today: `event_from_call` cannot, because §5.3 names
      the other `leg` and `C-6`'s `CallEvent::Bridged` / `CallEvent::Unbridged` deliberately do not
      carry it — and no driver composes them either, so `grep -rn 'EventKind::Bridged' crates/`
      finds the fixture and nothing else.
      → `tests/event_bridge.rs::a_coupled_call_tells_its_app_which_leg_the_media_crossed_to` and
      `::the_snapshot_follows_the_coupling_the_app_was_told_about`.
- [x] Whichever way it is settled, `tests/spec_tables.rs`'s
      `section_5_3_s_rows_are_reachable_through_the_bridge` names the answer in one place:
      `COMPOSED_BY_THE_DRIVER` says the driver composes these two, and a row that stays there with
      no driver composing it is a claim nothing checks. Either the list gets a producer behind it
      or the rows leave §5.3.
      → the rows got a producer; `COMPOSED_BY_THE_DRIVER` is down to three entries and the test's
      other branch now requires the arm.
- [ ] The full gate is green.

## Progress

- Backlog. Filed by `M-98` on 2026-08-09, which closed the same defect for `call.signal.metrics`
  and `call.signal.silence` and found these two next to them.

### 2026-08-09 — the rows got a producer; the answer is *the fact is the call's, the name is the app's*

**Failing-first, at `465605d`.** `cargo test -p sipx-app-protocol --all-features --test event_bridge`
on the two new tests, before any source change:

```
test a_coupled_call_tells_its_app_which_leg_the_media_crossed_to ... FAILED
test the_snapshot_follows_the_coupling_the_app_was_told_about ... FAILED
panicked at crates/sipx-app-protocol/tests/event_bridge.rs:78:28:
§5.3 has a row for Bridged and the bridge produced nothing
```

**The decision, and why it went this way rather than deleting the rows.** Both answers were live.
Deleting them loses:

- §6.2's `bridge` row says it completes with `call.bridged` and `unbridge` with `call.unbridged`, so
  removing the rows leaves two verbs completing with events the contract does not define;
- §5.2's `bridged` member is written by exactly these two events (`interpreter.rs`) and by nothing
  else, so removing them makes a snapshot member that can never be true — the same unreachable
  thing, one layer further down, which is what `M-98` warned the next story about.

So they get a producer. It went into `event_from_call` rather than a driver because **the fact is
the call's alone**: `UnbridgeCause::PeerEnded` is §6.2's "ended by `unbridge` **or either leg
ending**", and a driver watching its own instructions cannot tell that from a coupling it dropped
itself. What the call does not have is the *name*, and this function already had an answer for a
name only the caller knows — `instruction_id`. So `bridged_leg: Option<&str>` joins it, and the two
variants left `has_no_contract_event`. The reason they were in that set was a missing name, not a
missing event.

Putting the producer in the driver instead was checked and rejected on evidence: `sipx-app`'s host
refuses `Effect::Bridge` (`host.rs`, the phase-1 `_ => self.fail_effect()` arm), so a driver-side
producer would sit in a branch nothing reaches — the defect again, in a new place.

**The guard still bites.** Deleting the new `EventKind::Bridged` arm and re-running
`section_5_3_s_rows_are_reachable_through_the_bridge` gives:

```
§5.3 lists call.bridged and the bridge has no arm producing `EventKind::Bridged`, so no call can
reach it — add the arm, or name the row in `COMPOSED_BY_THE_DRIVER` with the reason the driver
composes it
```

**Owed `CHANGELOG.md` sentence** (the coordinator writes it; `event_from_call` is a *Supported*
surface, so a signature change owes migration guidance):

> `sipx_app_protocol::event_from_call` takes a third argument, `bridged_leg: Option<&str>`, and now
> produces §5.3's `call.bridged` and `call.unbridged` — which nothing could emit before. Callers
> that do not couple media pass `None`; a driver that performs `Effect::Bridge` passes the `leg`
> that effect carries, and holds the name until the resulting `CallEvent::Unbridged` is mapped.

**Left open**, filed as `M-103`: `call.dial.finished` is the same defect and is still open — the
only `EventKind::DialFinished` mentions in `src` are consumers, and `host.rs` refuses `Effect::Dial`
alongside `Effect::Bridge`. It is the last entry in `COMPOSED_BY_THE_DRIVER` with no producer behind
it. `sipx-app` therefore still emits neither coupling row: this story made them reachable, and the
host that could reach them is phase-2 work.

Gate row left unticked — the wave gate is the coordinator's to run.

## Notes

- The shape of the answer is a decision about *who knows the leg*. §6.2's `dial` creates a leg and
  the app names it; a bridge made by the host couples two calls the host already has ids for. So
  the producer is plausibly the driver rather than the bridge — but "plausibly the driver's" is what
  the `_ => return None` arm effectively said about `CallEvent::SignalMetrics` for two releases, and
  the lesson of `M-98` is that an unproduced row and an unreachable one are indistinguishable from
  the outside.
- `check-audio-claims.py` already records that `sipx-call` has nothing named bridge (`X-35`,
  `C-6`'s gap). This is the app-contract half of the same absence, and the two are worth settling
  together rather than separately.
