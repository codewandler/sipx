---
id: P-25
title: Bound a registration attempt
pillar: Phone
status: backlog
priority:
design:
epic: diagnostic-automation
areas: [sipx-cli, sipx-ua]
predicate:
announcement:
note: external review finding 4's actual consequence · X-110 made --timeout an explicit error on register, but nothing bounds the attempt
---

# Bound a registration attempt

## Goal

Give `sipx register` a bounded completion deadline, so a scheduled registration check against a dead
or black-holing registrar fails on a stated schedule instead of blocking for the SIP transaction
timeout.

## Acceptance

- [ ] `register` accepts an explicit completion deadline covering the whole attempt — resolution,
      connection, the initial transaction and any authentication retry — not just one transaction.
- [ ] A failing-first test proves a dead registrar returns on that deadline rather than at the RFC
      transaction timeout, in both text and JSON output, with the timeout distinguishable from a
      rejection and from a transport failure by exit status.
- [ ] The default is stated in `website/docs/reference/cli.md` alongside `dial`'s, and the two verbs
      agree on flag name and units.
- [ ] Cancellation drops and joins every lookup and attempt task within the deadline's bound; no
      registration binding is left half-created.

## Progress

- 2026-08-08: filed while writing the post-`rc.2` changelog boundary. External review finding 4
  reported `register --timeout` overshooting by roughly ten times. `X-110` addressed the *reporting*
  half — `--timeout` is now an explicit usage error on `register` rather than a silently swallowed
  flag — but `crates/sipx-cli/src/register.rs` still offers only `--expires`, so there remains no way
  to bound the attempt itself. The reported consequence, a scheduled check blocking roughly 32
  seconds against a dead registrar, is unfixed.

## Notes

- `P-17` did this for `dial` (bounded completion including cancellation cleanup); reuse its shape and
  its vocabulary rather than inventing a second one.
- `T-38`/`T-39`'s resolver already owns bounded resolution deadlines — this story is the transaction
  and retry half, and must compose with them rather than add a second competing clock.
