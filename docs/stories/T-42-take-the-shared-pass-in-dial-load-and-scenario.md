---
id: T-42
title: Take the shared candidate pass in dial, load and scenario
pillar: Transport
status: backlog
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

- [ ] `dial`, `load` and `scenario` walk candidates through `sipx_transport::destination::walk`
      rather than their own loops.
- [ ] `load` and `scenario` connection failures carry `candidates_attempted` /
      `candidates_resolved`, which today they do not — a failing-first test proves each.
- [ ] Each command's retry classification is unchanged by the move, proved per command rather than
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

## Notes

- `dial` carries a side-car `Unreachable { error, attempts }` because `sipx_call::Error` has no
  `ConnectionFailed` variant. The shared pass returns `Unreached<E>` and is generic over the error,
  so that side-car should collapse into it rather than being ported across.
- `crate::destination::with_attempts` is the reporting half and already emits both fields; the work
  is in producing an `Attempts` for the commands that currently produce none.
