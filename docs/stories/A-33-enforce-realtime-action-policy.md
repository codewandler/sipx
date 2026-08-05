---
id: A-33
title: Enforce schema idempotency and confirmation for model actions
pillar: Application
status: backlog
priority: 33
design: docs/designs/openai-realtime-phone.md
epic: openai-realtime-phone
areas: [testkit, app-sdk, call-control, policy, security, openai-realtime, m17]
predicate:
announcement:
note: after A-22 · application owns policy, timeouts and consequential-action confirmation
---

# Enforce schema idempotency and confirmation for model actions

## Goal

Put a deterministic application policy between every model request and phone mutation, including
strict validation, replay safety, deadlines and explicit confirmation for consequential operations.

## Acceptance

- [ ] Closed schemas reject unknown fields, wrong types, overlong values, invalid DTMF, unowned call
      IDs and transfer targets outside application-owned policy before any phone action runs.
- [ ] Function call IDs form a bounded idempotency store: a duplicate returns the prior correlated
      outcome and never repeats any model-exposed phone action.
- [ ] Every request has an explicit queue deadline, execution timeout, cancellation and terminal
      result; late completion cannot mutate a replaced session or ended call.
- [ ] A configurable confirmation policy identifies consequential actions, records who/what
      confirmed them, and refuses on absence, expiry or ambiguity; model text cannot self-confirm.
- [ ] Requested, accepted, refused, timed-out, cancelled and completed events are observable to the
      application with redacted arguments and stable refusal codes.
- [ ] Adversarial property tests cover schema confusion, duplicate/reordered calls, confirmation
      races, queue pressure and cross-call correlation, and the full gate is green.

## Progress

- Backlog. Extends A-22's deny-by-default host policy and gates A-32.
