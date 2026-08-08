---
id: P-27
title: Make every command exit a join barrier
pillar: Phone
status: ready
priority: 22
design:
epic: diagnostic-automation
areas: [sipx-cli]
predicate:
announcement:
note: P-25 added the join on register's deadline path only · every other exit still reports before its work is observably finished
---

# Make every command exit a join barrier

## Goal

Make the terminal record of every diagnostic command mean the same thing it means on `dial`: that
the work is observably finished, not merely that the result is known.

## Acceptance

- [ ] `register` joins its endpoint on every exit — success, rejection and transport failure — not
      only on the deadline path `P-25` added.
- [ ] A failing-first test proves no socket, task or timer outlives the terminal record, for each
      exit class of each long-running command.
- [ ] `--keep-alive` stops sending a redundant second REGISTER per invocation: it currently calls
      `register_candidates` and then `keep_registered()`, which registers again immediately.
- [ ] `--keep-alive` refreshes are bounded rather than governed only by the granted lease, matching
      the deadline `P-25` established for the initial attempt.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `P-25`'s adjacent findings. That story added `handle.shutdown()` on the
  timeout path because its acceptance asked for it, and deliberately left the rest: `register` never
  joins on a non-timeout failure or on success, so its counters are read from a still-running
  endpoint.

## Notes

- `P-25`'s `report_attempt_timeout` is the shape to follow, and `dial` already does this correctly.
- Reading counters after `shutdown()` is deliberate there; keep that ordering.
