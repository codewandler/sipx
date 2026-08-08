---
id: T-42
title: Take the shared candidate pass in dial, load and scenario
pillar: Transport
status: in-progress
priority: 3
design: docs/designs/endpoint-resolution.md
epic: endpoint-resolution
areas: [sipx-cli]
predicate:
announcement:
note: five hand-rolled candidate loops remain across dial, load and scenario; three of them count no attempts, so their connection failures cannot say how far they got
---

# Take the shared candidate pass in dial, load and scenario

## Goal

Route the remaining hand-rolled candidate loops through the shared serial pass, so every outbound
command walks the list the same way and reports how far it got in the same two fields.

## Acceptance

- [x] `dial`, `load` and `scenario` walk candidates through `sipx_transport::destination::walk`
      rather than their own loops.
- [x] `load` and `scenario` connection failures carry `candidates_attempted` /
      `candidates_resolved`, which today they do not — a failing-first test proves each.
- [x] Each command's retry classification is unchanged by the move, proved per command rather than
      assumed from the shape of the diff.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `P-31`'s adjacent findings. `P-31` extracted the serial pass into
  `sipx_transport::destination::walk` and moved `UserAgent::register_candidates` and `peers` onto
  it. Five loops were left behind, all of them using `.take(MAX_ATTEMPTS)` and none of them sharing
  a deadline the way the pass does:

  | Loop | Location | Counts attempts? |
  |---|---|---|
  | `dial_candidates` | `crates/sipx-cli/src/dial.rs` | yes |
  | `dial_early_candidates` | `crates/sipx-cli/src/dial.rs` | yes |
  | `run_attempt` | `crates/sipx-cli/src/load.rs` | **no** |
  | `run_generated_media_attempt` | `crates/sipx-cli/src/load.rs` | **no** |
  | scenario `dial` step | `crates/sipx-cli/src/scenario.rs` | **no** |

  The three that count nothing are the reason this is a defect and not a tidy-up: a `load` run whose
  target has four addresses reports `Connection refused` with no way to tell one dead host from a
  name where every address is dead — the exact gap `T-41` closed for `register` and `dial`.

- 2026-08-08: implemented on `impl/T-42`. All five loops are gone; `crate::destination::MAX_ATTEMPTS`
  is no longer re-exported to the commands, so nothing in `sipx-cli` can walk a candidate list by
  hand any more. `dial`'s side-car collapsed as the Notes below asked: `Unreachable` is now an alias
  for `Unreached<sipx_call::Error>`.

  **Failing-first, at `1c7d1c4`** (`cargo test -p sipx-cli --all-features --test cli`, this
  worktree's own target directory):

  ```
  test a_load_run_that_reaches_no_address_reports_what_it_attempted ... FAILED
  test a_scenario_dial_that_reaches_no_address_reports_what_it_attempted ... FAILED

  ---- a_load_run_that_reaches_no_address_reports_what_it_attempted stdout ----
  assertion `left == right` failed: signalling: every address behind the name was attempted, and
  the summary has to say so: {…,"outcomes":{"attempted":2,…},"schema":"sipx.load.v1",…}
    left: Null
   right: Number(3)

  ---- a_scenario_dial_that_reaches_no_address_reports_what_it_attempted stdout ----
  assertion `left == right` failed: every address behind the name was attempted, and the refusal
  has to say so: {…,"event":{"id":"dial-1","message":"transport: io: Connection refused (os error
  111)","type":"scenario.command.refused"},…}
    left: Null
   right: Number(3)
  ```

  Both now pass, alongside two classification guards that passed at the base and still do —
  `dial_and_scenario_reach_a_name_whose_first_address_is_dead` and
  `load_reaches_a_target_whose_first_address_is_dead`. That the guards were green at the base is
  worth recording: the hand-rolled loops did fall through a dead address, so what was missing was
  never the fallback but the report of how far it went.

  Three behaviours changed beyond the reporting, each deliberate:

  1. **`load` funds its pass from one `--timeout`.** Both of its loops previously gave every
     candidate a fresh copy of the setup deadline, so a three-address target could spend three
     times the number the operator typed on one call (`P-29`). `run_signalling_attempt` now takes
     the pass's remainder rather than `limits.setup_timeout`.
  2. **An expired `dial` pass is a timeout.** `walk` reports `Expired` without the last transport
     error, so a pass the deadline ended now exits `timeout` with the counts beside it, instead of
     exiting `failed` with the last address's refusal. That is what `register` and `peers` already
     do with an expired pass, and the deadline — not the address — is what ended it.
  3. **`load` reports the deepest failed pass**, since every admitted call walks the same resolved
     list. `null`, never zero, when no failed call walked one.

  Owed to `CHANGELOG.md` (not edited here — the coordinator owns that file): *`load` and `scenario`
  now report `candidates_attempted` and `candidates_resolved` when a connection failure ends a
  candidate pass, and `load` bounds all of a call's candidates by one `--timeout` rather than each.*

  The board is not regenerated here either; `T-43` and `T-44` were filed from this work and need it.

## Notes

- `dial` carries a side-car `Unreachable { error, attempts }` because `sipx_call::Error` has no
  `ConnectionFailed` variant. The shared pass returns `Unreached<E>` and is generic over the error,
  so that side-car should collapse into it rather than being ported across.
- `crate::destination::with_attempts` is the reporting half and already emits both fields; the work
  is in producing an `Attempts` for the commands that currently produce none.
