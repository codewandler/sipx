---
id: S-24
title: Learn who is registered, with the registration event package
pillar: Signalling
status: backlog
priority:
design: docs/designs/discovery.md
epic: discovery
areas: [sipx-ua, sipx-sip]
note: RFC 3680 is `partial` today — sipx parses the package and never subscribes to it
---

# Learn who is registered, with the registration event package

## Goal
Ask a registrar who is registered, and keep the answer current. The `discovery` epic's source for
the case where real infrastructure exists.

## Acceptance
- [ ] sipx subscribes to the `reg` event package (RFC 3680) at a registrar and turns the NOTIFY
      bodies into peers the epic's list can show. The package is already `partial` in the registry
      — `sipx-sip` parses it and nothing ever subscribes.
- [ ] The list stays current from subsequent NOTIFYs rather than being fetched once: a contact
      that registers or expires while the subscription is live changes what `sipx peers` prints.
- [ ] **A refusal is surfaced as a refusal.** A registrar may reasonably decline to enumerate its
      users, and RFC 3680 subscriptions are authorized for exactly that reason. A `403`, a `489` or
      a subscription that is never granted must produce a stated error — never a partial list
      presented as complete, and never a suggestion that discovery routes around authorization.
- [ ] Peers from this source are labelled as such in the output `P-5` defined, with their age.
- [ ] The RFC registry row for 3680 moves off `partial` or says precisely what is still missing.
- [ ] Failing-first test: `a_contact_that_registers_while_subscribed_appears_in_the_list`.

## Progress
- Not started. Needs `P-5` for somewhere to put the result.

## Notes
- Third story of the `discovery` epic; see [the design](../designs/discovery.md).
- **Subscribing to someone else's registrar is deliberately not the same as being one.** sipx
  serving registrations for other endpoints would be a PBX, which the vision names as a non-goal.
  This gets the same information without becoming the infrastructure.
- Builds on the subscription machinery already shipped — the stack "serves subscriptions to what
  its dialogs and registrations are doing" — so the new work is the client half and the `reg`
  package's semantics, not a new event framework.
- RFC 6665 (`SIP-Specific Event Notification`) is `partial` and is this story's substrate; check
  whether anything it still lacks blocks a long-lived subscription before starting.
