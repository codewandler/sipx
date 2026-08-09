---
id: M-108
title: Tell an app that a dialled leg went away
pillar: Media
status: backlog
priority: 30
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

- [ ] A failing-first test proves an app is told when a leg it dialled ends. Today `dial` a leg,
      let it answer, have the far end send BYE: the driver's task observes the ending and the app
      sees no event and an unchanged snapshot.
- [ ] §5.2's `legs` is correct after that ending. `interpreter.rs`'s `apply_to_snapshot` rewrites
      `legs` in exactly one arm — `EventKind::DialFinished` — which fires once, when the dial
      resolves. Nothing else ever removes a leg.
- [ ] The contract says which event carries it, in `docs/specs/app-contract.md`. §5.3 has no row
      for this: `call.unbridged` names a coupling ending and §6.2 requires a `bridge` to have been
      made, so it is not the answer for a leg nobody bridged. Either a row is added, or the section
      states in normative prose that an unbridged leg's ending is not reported and why.
- [ ] The full gate is green.

## Notes

- `M-103`'s driver holds each answered leg in `DialTasks`, served by `sipx_call::serve_until` on its
  own task. That task already observes the ending — `Served::Remote` names the cause — so what is
  missing is a channel back to the actor and a §5.3 row to put on it, not an observation.
- The neighbouring decision is `M-99`'s: `call.unbridged` deliberately carries no cause, because
  "that leg's own `call.ended` is where an app reads it". A leg this host dialled has no `call.ended`
  of its own — it is not a call the app is bound to — so that sentence does not cover this case and
  the section should say so either way.
