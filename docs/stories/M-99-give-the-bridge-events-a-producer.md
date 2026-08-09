---
id: M-99
title: Give `call.bridged` and `call.unbridged` a producer
pillar: Media
status: backlog
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

- [ ] A failing-first test proves an app-protocol client is told when the media coupling on its
      call changed. Nothing produces either row today: `event_from_call` cannot, because §5.3 names
      the other `leg` and `C-6`'s `CallEvent::Bridged` / `CallEvent::Unbridged` deliberately do not
      carry it — and no driver composes them either, so `grep -rn 'EventKind::Bridged' crates/`
      finds the fixture and nothing else.
- [ ] Whichever way it is settled, `tests/spec_tables.rs`'s
      `section_5_3_s_rows_are_reachable_through_the_bridge` names the answer in one place:
      `COMPOSED_BY_THE_DRIVER` says the driver composes these two, and a row that stays there with
      no driver composing it is a claim nothing checks. Either the list gets a producer behind it
      or the rows leave §5.3.
- [ ] The full gate is green.

## Progress

- Backlog. Filed by `M-98` on 2026-08-09, which closed the same defect for `call.signal.metrics`
  and `call.signal.silence` and found these two next to them.

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
