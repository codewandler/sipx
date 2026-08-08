---
id: P-30
title: Bound the first NOTIFY in a registrar subscription
pillar: Phone
status: ready
priority: 34
design:
epic: diagnostic-automation
areas: [sipx-cli, sipx-ua]
predicate:
announcement:
note: peers --registrar waits Timer N — 64*T1, 32 seconds — with no operator control · the dominant unbounded wait once resolution is bounded
---

# Bound the first NOTIFY in a registrar subscription

## Goal

Give `peers --registrar` an operator-stateable bound on how long it waits for the first NOTIFY,
which is now the longest thing it can do without saying so.

## Acceptance

- [x] The wait for the first NOTIFY is bounded by a stated deadline rather than by the event
      client's Timer N (64·T1, 32 seconds), and the bound is operator-controllable.
- [x] A failing-first test proves a registrar that accepts the SUBSCRIBE and never notifies returns
      on the stated deadline, distinguishable in text, JSON and exit status from a refused
      subscription and from a transport failure.
- [ ] Cancellation drops and joins the subscription without leaving a binding the registrar still
      believes in.
- [ ] The published reference states the bound and its default alongside the other command
      deadlines, checked rather than prose.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `P-26`'s adjacent findings. Having bounded resolution across every command,
  the dominant unbounded wait in `peers --registrar` is the first NOTIFY: it inherits Timer N from
  `event_client` with no operator control.

- 2026-08-08: **PARTIAL.** The first NOTIFY is bounded at 20 s — matching `register`'s default
  attempt deadline, so the two commands answer on the same clock — and measured failing-first:
  **32.015 s before, 20.02 s after**, which is Timer N (64·T1) observed directly rather than
  inferred. A registrar that accepts the subscription and never notifies now exits `Timeout` with a
  message naming the bound that expired, distinct from `Termination::NoInitialNotify`, which is the
  registrar saying it will not notify.
  Two rows stay open. The bound is a constant, not yet **operator-stateable** — `peers` has no
  attempt-deadline flag, and adding one reopens `P-26`'s judgement that `--expires` is the
  resolution ceiling, which is a decision about the command's whole clock rather than this wait.
  Cancellation joining without leaving a binding the registrar believes in is untested here.

## Notes

- `P-26` used `--expires` as the resolution ceiling for `peers` because it is the only duration the
  command states. If this story adds a real attempt deadline, revisit that reading — `P-26`'s
  deviation 2 records the alternative and the one line that implements it.
