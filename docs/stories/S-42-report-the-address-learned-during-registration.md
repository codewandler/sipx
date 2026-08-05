---
id: S-42
title: Report the address learned during registration
pillar: Signalling
status: done
priority: 10
design: docs/designs/stack-comparison.md
epic: registration-observation
areas: [sipx-ua, sipx-transport, m13, parity-wave-1]
predicate:
announcement:
note: discovered by X-97 · preserve received/rport observation in the typed registration outcome
---

# Report the address learned during registration

## Goal

Let a registering endpoint observe the public address reported by the registrar without parsing the
wire response again or confusing that observation with a routable Contact policy.

## Acceptance

- [x] A spec cites RFC 3261 §18.2.1 and RFC 3581 and defines how `received` and `rport` on the top
      response Via become an optional typed address on a successful registration outcome.
- [x] Missing, malformed, contradictory and non-IP observations have explicit typed outcomes; none
      can panic or replace the registrar-granted lease, routes, GRUU, Outbound or push state.
- [x] UDP and connection-oriented registration tests cover a learned address and the absent case,
      and authentication retry does not lose the final response's observation.
- [x] The API states that the learned value is an observation, not permission to rewrite future
      Contacts or media addresses automatically.
- [x] RFC registry evidence is updated and `./scripts/gate.py` is green.

## Progress

- The independent `registration-observation` branch carries the spec, typed registrar and
  `UserAgent` surfaces, malformed-input vectors, UDP/TCP runtime coverage, final-auth-response
  coverage, public guide, RFC evidence and generated maturity/compliance changes.
- Focused `sipx-ua` check, clippy and test suites are green. The corrected full workspace gate passed
  all 36 steps after the parallel M13 branches merged.
