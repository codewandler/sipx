---
id: S-17
title: Implement the dialog and registration event packages
pillar: Signalling
status: backlog
priority:
design:
epic: conformance
areas: [sipx-ua, sipx-call]
note: M8 · RFC 4235 + 3680 · blocked by S-13
---

# Implement the dialog and registration event packages

## Goal
The two event packages that report state sipx already keeps: which dialogs exist and what they
are doing (`dialog`, RFC 4235), and what is registered against an address of record (`reg`,
RFC 3680). Together they are what a busy-lamp field on a desk phone is actually subscribing to.

## Acceptance
- [ ] Both packages register with `S-13`'s framework by name; neither reaches into the
      subscription store directly.
- [ ] The `dialog` package emits `dialog-info` documents whose `version` increases monotonically
      per subscription, and whose `state` follows RFC 4235 §3 — `trying`, `proceeding`, `early`,
      `confirmed`, `terminated`.
- [ ] A full state document is sent on subscription (`state="full"`) and partial documents
      thereafter (`state="partial"`), because a watcher that joined mid-call must not have to
      infer what it missed.
- [ ] The `reg` package emits `reginfo` documents with per-contact state and the event that
      changed it (`registered`, `refreshed`, `expired`, `unregistered`).
- [ ] A subscription to either package terminates cleanly when the thing it watches disappears,
      with `Subscription-State: terminated;reason=noresource` rather than by timing out.
- [ ] Failing-first test: `a_watcher_sees_a_dialog_reach_confirmed_and_then_terminate`.

## Progress
- Not started. Blocked by `S-13`: without the framework there is nothing to register a package
  with.

## Notes
- These come before presence (`S-18`) deliberately. Both report state the stack *already has* —
  the dialog store and the registration lease — so they exercise the framework without also
  needing a state model of their own. Presence needs somewhere for presence to come from, which
  is a different question.
- RFC 4235 §3.2's version rule is per-subscription, not per-dialog. Two watchers of the same
  dialog get their own counters, and conflating them is the bug this package invites.
