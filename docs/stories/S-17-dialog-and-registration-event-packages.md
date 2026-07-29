---
id: S-17
title: Implement the dialog and registration event packages
pillar: Signalling
status: done
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
- [x] Both packages register with `S-13`'s framework by name; neither reaches into the
      subscription store directly.
- [x] The `dialog` package emits `dialog-info` documents whose `version` increases monotonically
      per subscription, and whose `state` follows RFC 4235 §3 — `trying`, `proceeding`, `early`,
      `confirmed`, `terminated`.
- [x] A full state document is sent on subscription (`state="full"`) and partial documents
      thereafter (`state="partial"`), because a watcher that joined mid-call must not have to
      infer what it missed.
- [x] The `reg` package emits `reginfo` documents with per-contact state and the event that
      changed it (`registered`, `refreshed`, `expired`, `unregistered`).
- [x] A subscription to either package terminates cleanly when the thing it watches disappears,
      with `Subscription-State: terminated;reason=noresource` rather than by timing out.
- [x] Failing-first test: `a_watcher_sees_a_dialog_reach_confirmed_and_then_terminate`.

## Progress
- Done. `DialogWatch` (RFC 4235) and `RegistrationWatch` (RFC 3680), each holding one watcher's
  view and producing the documents for it. Neither reaches into the subscription store: a package
  is a name and a body, and `S-13`'s framework does everything else.
- **The first document is `full` and the rest are `partial`** (§4.1). A watcher that joined
  mid-call is given the whole picture once and told about changes after that; sending only changes
  from the start leaves it inferring a state nobody ever described.
- **The version is per subscription, not per resource** (§4.1). Two watchers of the same dialogs
  each count from zero — sharing a counter would make one of them see gaps it cannot explain. It
  saturates rather than wraps, because a counter returning to zero looks like a new subscription.
- The five dialog states are the RFC's own and are not collapsed: a watcher renders `early` as
  "ringing" and `confirmed` as "on a call", so merging them is a busy-lamp field that lights at the
  wrong moment. Same reasoning keeps `expired` and `unregistered` apart in the `reg` package —
  both mean gone, and *why* is what a display says.
- **XML metacharacters are escaped**, which is not cosmetic: a SIP URI can carry `&` in its
  parameters, and one unescaped makes the whole document unparseable — a watcher then sees nothing
  at all rather than a slightly wrong dialog.
- A test caught its own trap on the way: `version="` appears in the XML declaration before it
  appears on the `dialog-info` element, so reading "the first version" reads the wrong one.
- Mutation-tested: sending every document as `full`, never advancing the version, not escaping, and
  reporting an expired contact as still bound.
- **The seam to `S-13` is tested from outside, in `crates/sipx-ua/tests/packages.rs`.** Two of the
  acceptance criteria live only at that join and cannot be asserted on either side alone: that each
  package is reachable under the name a subscriber puts in `Event`, and that a subscription ends
  with `reason=noresource` when its resource goes. The subscribe uses the **literal** `dialog` and
  `reg` tokens rather than `DialogWatch::package()` on both sides — registering and subscribing
  through the same expression passes whatever it returns and tests the notifier's string comparison
  instead of the package's name. Mutating the name to `dialog-info` and the reason token to
  `timeout` each fail it.
- `noresource` is asserted to be *distinct from lapsing*: the test also checks `expire()` reports
  nothing, because a subscription left to time out tells a busy-lamp field nothing until its expiry
  runs out — the lamp stays lit for a line that is gone.
- A contact expiring is reported **in** a document, not as the resource disappearing. The address of
  record still exists with no contacts bound; conflating the two would terminate a subscription that
  should have carried on reporting an empty registration.

## Notes on what is not here
- The two packages produce documents; wiring them to sipx's *live* dialog store and registration
  lease is the application's join, and deliberately so — a package that reached into the call layer
  would make `sipx-ua` depend on `sipx-call`, reversing the dependency direction the workspace is
  built on.

## Notes
- These come before presence (`S-18`) deliberately. Both report state the stack *already has* —
  the dialog store and the registration lease — so they exercise the framework without also
  needing a state model of their own. Presence needs somewhere for presence to come from, which
  is a different question.
- RFC 4235 §3.2's version rule is per-subscription, not per-dialog. Two watchers of the same
  dialog get their own counters, and conflating them is the bug this package invites.
