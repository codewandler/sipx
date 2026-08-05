---
id: S-24
title: Learn who is registered, with the registration event package
pillar: Signalling
status: done
priority: 7
design: docs/designs/discovery.md
epic: discovery
areas: [sipx-ua, sipx-sip, parity-wave-1]
note: live bounded UAC and registration-event discovery are integration-gate proven
---

# Learn who is registered, with the registration event package

## Goal
Ask a registrar who is registered, and keep the answer current. The `discovery` epic's source for
the case where real infrastructure exists.

## Acceptance
- [x] sipx subscribes to the `reg` event package (RFC 3680) at a registrar and turns the NOTIFY
      bodies into peers the epic's list can show. The package is already `partial` in the registry
      — `sipx-sip` parses it and nothing ever subscribes.
- [x] The list stays current from subsequent NOTIFYs rather than being fetched once: a contact
      that registers or expires while the subscription is live changes what `sipx peers` prints.
- [x] **A refusal is surfaced as a refusal.** A registrar may reasonably decline to enumerate its
      users, and RFC 3680 subscriptions are authorized for exactly that reason. A `403`, a `489` or
      a subscription that is never granted must produce a stated error — never a partial list
      presented as complete, and never a suggestion that discovery routes around authorization.
- [x] Peers from this source are labelled as such in the output `P-5` defined, with their age.
- [x] The RFC registry row for 3680 moves off `partial` or says precisely what is still missing.
- [x] Failing-first test: `a_contact_that_registers_while_subscribed_appears_in_the_list`.

## Progress
- `RegistrationConsumer` is the concrete `reg` package policy behind S-38. It parses bounded XML,
  requires an authoritative version-zero full document, applies exact-next partial changes
  atomically, retains current typed contacts and rejects gaps, duplicate identities, DTD/entity
  input and capacity overflow without publishing a truncated snapshot.
- `EventSubscriptions` records the monotonic observation instant. `sipx peers --registrar` waits for
  the first complete snapshot, optionally observes later NOTIFYs for `--watch`, reports source and
  age, and maps 403, 489 and missing initial NOTIFY to scriptable non-zero outcomes. An explicit
  `--book` merges local facts; no implicit book can disguise a refused registrar result.
- The live failing-first test observes a registration arrive and expire over real SIP transactions,
  then verifies unsubscribe cleanup. Focused compilation and tests passed on the epic branch, then
  the corrected integration gate passed all 36 steps.

## Notes
- Third story of the `discovery` epic; see [the design](../designs/discovery.md).
- **Subscribing to someone else's registrar is deliberately not the same as being one.** sipx
  serving registrations for other endpoints would be a PBX, which the vision names as a non-goal.
  This gets the same information without becoming the infrastructure.
- Builds on the subscription machinery already shipped — the stack "serves subscriptions to what
  its dialogs and registrations are doing" — and on `S-38`'s reusable event client. This story owns
  only the `reg` package-to-peer-list policy, not a second subscriber implementation.
- RFC 6665 (`SIP-Specific Event Notification`) is `partial` and is this story's substrate; check
  whether anything it still lacks blocks a long-lived subscription before starting.
