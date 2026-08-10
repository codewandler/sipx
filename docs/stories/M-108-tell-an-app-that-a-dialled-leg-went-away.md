---
id: M-108
title: Tell an app that a dialled leg went away
pillar: Media
status: done
priority: 54
design: docs/designs/app-sdk.md
epic: app-sdk
areas: [sipx-app, sipx-app-protocol, app-sdk, m16]
predicate:
announcement:
note: filed by M-103 · §5.2's `legs` is only ever rewritten by `call.dial.finished`, so a leg that answers and later hangs up stays in the snapshot forever
---

# Tell an app that a dialled leg went away

## Goal

Close the gap `M-103` left visible the moment `dial` started working: a second leg that **answers**
and is later ended by the far end is reported to the app by nothing, and stays in §5.2's `legs` as
`answered` for the rest of the call.

## Acceptance

- [x] A failing-first test proves an app is told when a leg it dialled ends. Today `dial` a leg,
      let it answer, have the far end send BYE: the driver's task observes the ending and the app
      sees no event and an unchanged snapshot.
- [x] §5.2's `legs` is correct after that ending. `interpreter.rs`'s `apply_to_snapshot` rewrites
      `legs` in exactly one arm — `EventKind::DialFinished` — which fires once, when the dial
      resolves. Nothing else ever removes a leg.
- [x] The contract says which event carries it, in `docs/specs/app-contract.md`. §5.3 has no row
      for this: `call.unbridged` names a coupling ending and §6.2 requires a `bridge` to have been
      made, so it is not the answer for a leg nobody bridged. Either a row is added, or the section
      states in normative prose that an unbridged leg's ending is not reported and why.
- [x] The full gate is green.

## Progress

- Backlog. Filed by `M-103` on 2026-08-09, from the case its own last section left open.

### 2026-08-10 — the row exists, and the driver is what produces it

**Failing-first, at the merge base `c43ebde`.** `crates/sipx-app/tests/host.rs`'s
`a_dialled_leg_the_far_end_hangs_up_is_reported_to_the_app` dials a peer that answers, waits for the
ACK, and hangs up. Run in this worktree at that commit, before any other change:

```
$ cargo test -q -p sipx-app --test host --all-features -- a_dialled_leg
running 1 test
a_dialled_leg_the_far_end_hangs_up_is_reported_to_the_app --- FAILED

thread 'a_dialled_leg_the_far_end_hangs_up_is_reported_to_the_app' panicked at
crates/sipx-app/tests/host.rs:443:19:
the app is never sent an event for the leg's own ending

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 9 filtered out
```

The three assertions **before** that point already passed at the base, which is the rest of the
story's claim in one run: the `dial` resolved `answered`, §5.2 listed the leg, and then nothing —
the leg's ending reached no app and moved no snapshot.

**A row, not prose.** §5.3 gained `call.leg.ended` with `leg` and `cause`. The alternative the
Acceptance allowed — say in normative prose that an unbridged leg's ending is not reported — was
rejected because it cannot be made true of the *snapshot*: §5.2's `legs` is authoritative under §2,
and the only thing that may stop listing a leg is an event. Silence would have left the contract
promising a current snapshot while shipping a stale one.

**Why it carries a cause where `call.unbridged` does not.** `M-99` left that event causeless on the
grounds that *"that leg's own `call.ended` is where an app reads it"*. A leg this host placed is not
a call the app is bound to and has no envelope stream, so there is no other place — the section now
says so. The vocabulary is this row's own (`remote · timeout · error`) rather than `call.ended`'s
five words, because two of those cannot be true of a leg: `hangup` is *the app asked for it* and
§6.2 has no verb to ask with, and `rejected{status}` is a refused invitation, which is a
`call.dial.finished` outcome and not an ending.

**Where the producer went.** `crates/sipx-app/src/host.rs`. `EstablishedLeg::serve` already observed
the ending and threw the result away; it now returns it, `leg_end_cause` maps it, and the leg task
reports it on the **same channel as its own `call.dial.finished`** — one mpsc, one task, so the
ending can never overtake the resolution that put the leg in the snapshot. `DialedLeg` became
`LegReport::{Resolved, Ended}` for that reason and not for tidiness. An ending *this* side caused
(`Served::Interrupted`/`Local`) is deliberately not reported: the only thing that stops a leg is
`DialTasks::finish`, which runs while the actor is finishing, so the app has already been told the
call is over.

**What the row is held to.** `COMPOSED_BY_THE_DRIVER` gains
`("call.leg.ended", "crates/sipx-app/src/host.rs", DRIVER)`, so the same assertion `M-103` added
applies: the named file must contain the `EventKind::LegEnded` construction. Vector **AC-10** is the
interpreter half — a leg listed `answered` until the ending and gone after it, with the running
program untouched — and a unit test in `host.rs` holds every word of the `cause` vocabulary against
an arm that can produce it, so the enumeration cannot grow a word nothing sends.

**`DialTasks::finish` now drains while joining.** A leg whose far end hangs up in the same breath as
the stop still has a report to send, and the actor has stopped reading; on a full channel that task
would never join, which is the leak `AGENTS.md`'s bounded-background-work rule names.

**Owed CHANGELOG sentence** (the coordinator writes it): *An app is now told when a leg it dialled
goes away: §5.3's new `call.leg.ended` reports an answered outbound leg's ending with its cause, and
§5.2's `legs` stops listing it — until now a leg that answered stayed in the snapshot for the rest
of the call.*

**Not done here:** the gate row. The coordinator runs one gate for the wave; this worktree ran
`cargo test -p sipx-app -p sipx-app-protocol --all-features`, `cargo clippy` on both crates,
`cargo fmt --all`, `check-provenance.sh`, `check-fixed-sleep.py --check`, `check-docs-links.py`,
`sync-website.py --check`, `check-app-surface.py --check` and `test-app-surface.py`, all clean.

## Notes

- `M-103`'s driver holds each answered leg in `DialTasks`, served by `sipx_call::serve_until` on its
  own task. That task already observes the ending — `Served::Remote` names the cause — so what is
  missing is a channel back to the actor and a §5.3 row to put on it, not an observation.
- The neighbouring decision is `M-99`'s: `call.unbridged` deliberately carries no cause, because
  "that leg's own `call.ended` is where an app reads it". A leg this host dialled has no `call.ended`
  of its own — it is not a call the app is bound to — so that sentence does not cover this case and
  the section should say so either way.

- 2026-08-10: closed at the wave gate — 51 steps, all green, on the merged tree.
