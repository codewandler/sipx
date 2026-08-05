---
id: S-40
title: Surface application-owned dialog requests
pillar: Signalling
status: in-progress
priority: 4
design: docs/designs/dialog-extensions.md
epic: dialog-extensions
areas: [sipx-call, sipx-ua, m13, parity-wave-1]
predicate:
announcement:
note: authenticated INFO, MESSAGE and admitted extension methods without bypassing dialog invariants
---

# Surface application-owned dialog requests

## Goal

Let an application send and answer methods whose semantics it owns on a live dialog while preserving
the stack's specialized handling of session, transfer and teardown methods.

## Acceptance

- [ ] Incoming INFO, MESSAGE and explicitly admitted extension methods become a typed call event with
      method, validated headers, bounded body and a transaction-backed response capability.
- [ ] The response capability permits exactly one final response; drop or timeout produces a defined
      bounded refusal and releases the server transaction.
- [ ] The outbound API derives remote target, route set, dialog identifiers and monotonically
      increasing CSeq from the dialog; it does not accept a prebuilt request that can contradict them.
- [ ] 401/407 challenges reuse existing dialog credentials and tests cover INFO and MESSAGE in both
      directions plus one `Method::Other` value.
- [ ] OPTIONS, BYE, re-INVITE, UPDATE, REFER and NOTIFY stay on specialized paths, and tests prove the
      generic API cannot intercept or forge them.
- [ ] Invalid dialog state, body over limit and unsupported body semantics return typed errors without
      panic or partial send.
- [ ] Applicable RFC registry rows are updated and `./scripts/gate.py` is green.

## Progress

- In progress on the independent `dialog-extensions` epic branch.
