---
id: P-31
title: Walk every candidate in peers
pillar: Phone
status: ready
priority: 3
design: docs/designs/endpoint-resolution.md
epic: endpoint-resolution
areas: [sipx-cli]
predicate:
announcement:
note: peers --registrar resolves and then takes first() only, so a registrar whose first address is dead is unreachable from peers while register recovers
---

# Walk every candidate in peers

## Goal

Make `peers --registrar` try the resolved candidates the way every other outbound command does,
instead of giving up after the first address.

## Acceptance

- [ ] `peers` walks the resolved candidate list under its own deadline, so a registrar whose first
      address refuses is still reached — matching `register`, `dial` and `load`.
- [ ] A failing-first test proves a name whose first address is dead and whose second answers is
      reachable from `peers`, and is not today.
- [ ] The failure report carries the same `candidates_attempted` / `candidates_resolved` fields
      `T-41` established, so a script reads one shape across commands.
- [ ] The serial pass is not written a fifth time: reuse the helper `register` now delegates to.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `T-41`'s adjacent findings and verified — `crates/sipx-cli/src/peers.rs`
  resolves and then uses `first()`, never walking the list. This is a missing retry rather than a
  reporting gap: `register` recovers from a dead first address and `peers` does not, against the
  same registrar.

- 2026-08-08: **deferred out of rc.8 after reading the code.** Unlike `register`, which reuses one
  endpoint across candidates, `peers` selects an address *before* it binds: the chosen transport
  configures `TransportConfig`, which binds the endpoint, which the dispatcher and subscription are
  built on. Walking candidates therefore means retrying bind, dispatch and the first NOTIFY per
  candidate, not just re-sending a request — roughly the whole 80-line setup block restructured. That
  is worth doing properly rather than rushing; the defect is real and unchanged.

- 2026-08-08: **attempted and reverted.** The refactor is right and it compiled: the per-candidate
  work — transport configuration, bind, dispatcher, subscription and first notification — extracted
  into `attempt_candidate`, looped over the resolved list under `MAX_ATTEMPTS`, retrying only on a
  connection failure since a refusal or an authentication failure is the registrar speaking and a
  second address would not change it. All 93 `sipx-cli` tests passed.
  It was reverted because the **failing-first proof could not be finished**: the multi-candidate
  test needs the first candidate to *refuse*, and nothing refuses a UDP connection — so it needs a
  TCP registrar fixture that accepts, reads and responds, which the suite does not have. Landing a
  restructure of the `peers` setup path with only "the existing tests still pass" behind it is the
  shape this project's discipline exists to prevent. The `SPREAD` fixture name and its three
  loopback addresses are the right starting point for whoever picks this up.

## Notes

- `T-41` moved the serial pass into `UserAgent::register_candidates`. `load` and `scenario` still
  hand-roll their own — the loop now exists four times across three files and only two of them
  count attempts. This story should take the helper rather than add a fifth copy.
- `P-26` made `--expires` the resolution ceiling for `peers`, which was a judgement call recorded
  in that story; if this work gives `peers` a real attempt deadline, revisit that reading.
