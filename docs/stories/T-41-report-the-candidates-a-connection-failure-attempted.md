---
id: T-41
title: Report the candidates a connection failure attempted
pillar: Transport
status: ready
priority: 27
design: docs/designs/endpoint-resolution.md
epic: endpoint-resolution
areas: [sipx-transport, sipx-cli]
predicate:
announcement:
note: the spec's ConnectionFailed promises how many candidates were attempted; only the last error reaches the operator
---

# Report the candidates a connection failure attempted

## Goal

Tell an operator how many resolved candidates were tried before a connection failure, which is the
difference between "the name resolves to one dead host" and "every address behind this name is
unreachable".

## Acceptance

- [ ] A connection failure reports the number of candidates attempted alongside the last transport
      error, in text and JSON, as `docs/specs/sip-target-resolution.md` §8's
      `ConnectionFailed { attempted, last_error }` already promises.
- [ ] A failing-first test covers a single-candidate name and a multi-candidate name, asserting the
      count differs and that a serial retry budget exhausting is distinguishable from one host
      refusing.
- [ ] No new exit code and no renamed JSON field; the published exit table and contract table stay
      as `T-39` left them.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `T-39`'s adjacent findings — the spec's §8 vocabulary promises `attempted`
  and nothing carries it to the operator.
