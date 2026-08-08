---
id: P-25
title: Bound a registration attempt
pillar: Phone
status: done
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

- [x] `register` accepts an explicit completion deadline covering the whole attempt — resolution,
      connection, the initial transaction and any authentication retry — not just one transaction.
- [x] A failing-first test proves a dead registrar returns on that deadline rather than at the RFC
      transaction timeout, in both text and JSON output, with the timeout distinguishable from a
      rejection and from a transport failure by exit status.
- [x] The default is stated in `website/docs/reference/cli.md` alongside `dial`'s, and the two verbs
      agree on flag name and units.
- [x] Cancellation drops and joins every lookup and attempt task within the deadline's bound; no
      registration binding is left half-created.

## Progress

- 2026-08-08: filed while writing the post-`rc.2` changelog boundary. External review finding 4
  reported `register --timeout` overshooting by roughly ten times. `X-110` addressed the *reporting*
  half — `--timeout` is now an explicit usage error on `register` rather than a silently swallowed
  flag — but `crates/sipx-cli/src/register.rs` still offers only `--expires`, so there remains no way
  to bound the attempt itself. The reported consequence, a scheduled check blocking roughly 32
  seconds against a dead registrar, is unfixed.

- 2026-08-08: **readiness audit — ready as written**, no blocking gaps found.

- 2026-08-08: **implemented.** `register --timeout <S>` defaults to 20 seconds, carries `dial`'s
  flag name and units, and is one budget over the whole attempt. `Attempt` in
  `crates/sipx-cli/src/register.rs` starts the clock before resolution and funds each phase from
  what is left of it: the resolver is built by `Resolver::within`, which clamps `T-38`/`T-39`'s
  per-lookup and whole-resolution deadlines under the attempt's rather than running a second clock
  beside them, and each resolved candidate gets the remainder rather than a fresh copy of the
  budget. `UserAgent::register_within` in `sipx-ua` owns the transaction half — one bound over the
  initial REGISTER, the authenticated retry and a stale-nonce retry after it — and discards the
  GRUUs and push support an abandoned attempt learned, so nothing claims a binding it does not
  have. `UserAgent::woken_within` bounds `--wake`'s §4.1.3 refresh the same way. Expiry joins what
  the attempt owned through endpoint shutdown before reporting `status=timeout`, `aor`, `error`,
  `registration_limit_ms`, `registration_elapsed_ms` and `cleanup_ms` in both formats, exit 5.

  Two consequences worth naming. `UserAgent::attempt` now keeps a concrete transport failure
  instead of reporting `NoResponse`: without that a refused TCP connection exited 5 exactly like a
  deadline, and the Acceptance asks for those to be told apart by exit status. And
  `docs/specs/diagnostic-phone.md` gained §3.5 for the deadline's normative shape.

  Failing-first: `register_bounds_a_black_holing_registrar_on_its_own_deadline` and
  `register_tells_a_deadline_a_refusal_and_a_transport_failure_apart_by_exit_status` in
  `crates/sipx-cli/tests/cli.rs`, both exiting 2 with "unexpected argument '--timeout' found" at the
  merge base. `a_bounded_attempt_covers_the_authentication_retry` in
  `crates/sipx-ua/tests/register.rs` holds the two-transaction case under a paused clock.

  Left open deliberately: `--keep-alive`'s refreshes after a bounded first attempt are still
  governed by the granted lease alone, and `keep_registered` re-registers once unbounded before
  its loop. Both are pre-existing and named in the spec rather than changed here.

- 2026-08-08: closed against a green gate — `./scripts/gate.py` reported **40 steps, all green** on `main` at `1256b8e`. An earlier run on the same tree failed two `sipx-cli` audio tests; both pass in isolation and in the full 83-test `cli.rs` binary, and `M-59` independently hit the same class on a sibling test while five other checkouts were building on this shared box. `X-118` owns that flakiness.

## Notes

- `P-17` did this for `dial` (bounded completion including cancellation cleanup); reuse its shape and
  its vocabulary rather than inventing a second one.
- `T-38`/`T-39`'s resolver already owns bounded resolution deadlines — this story is the transaction
  and retry half, and must compose with them rather than add a second competing clock.
