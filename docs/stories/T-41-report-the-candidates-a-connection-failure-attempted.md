---
id: T-41
title: Report the candidates a connection failure attempted
pillar: Transport
status: in-progress
priority: 5
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

- [x] A connection failure reports the number of candidates attempted alongside the last transport
      error, in text and JSON, as `docs/specs/sip-target-resolution.md` §8's
      `ConnectionFailed { attempted, last_error }` already promises.
- [x] A failing-first test covers a single-candidate name and a multi-candidate name, asserting the
      count differs and that a serial retry budget exhausting is distinguishable from one host
      refusing.
- [x] No new exit code and no renamed JSON field; the published exit table and contract table stay
      as `T-39` left them.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `T-39`'s adjacent findings — the spec's §8 vocabulary promises `attempted`
  and nothing carries it to the operator.
- 2026-08-08: implemented. `sipx_transport::destination::Attempts` is the shared value — attempted,
  resolved, the `MAX_ATTEMPTS` budget over both, and `exhausted()` — and it is deliberately a *pair*
  of numbers rather than one: `P-26` makes a command's deadline the ceiling over the whole serial
  pass, so `attempted` is attempted-so-far and only `attempted == min(resolved, MAX_ATTEMPTS)` says
  the name itself is ruled out.

  Surfaced on both sides, which is what `library-parity` asked for. `sipx_ua::Error::ConnectionFailed`
  carries the pair beside the last transport error, and `UserAgent::register_candidates` is the
  serial pass that produces it — moved out of `sipx-cli`, so an application gets the candidate
  order, the shared budget and the count without reproducing any of it, and `sipx-cli`'s
  `register_candidates` is now the command-layer part alone (restating the deadline with the number
  the caller typed). `dial` carries the same pair beside `sipx_call::Error`, which knows nothing of
  the list the command walked; `crate::destination::with_attempts` renders both commands' fields so
  they cannot drift.

  `T-39`'s taxonomy is untouched: no new exit code, no renamed field, and the connection-failure
  message still opens with the transport cause rather than `target resolution failed:` — the count
  is appended to it (`after attempting 3 of 3 candidates`) and reported separately as
  `candidates_attempted` / `candidates_resolved`.

  Failing-first at `8803660`: the CLI process proof asserted a `candidates_attempted` that the
  report did not carry at all. Gate not run here — the coordinator runs one per wave. Focused
  verification green: `cargo test -p sipx-transport -p sipx-cli -p sipx-ua --all-features` (0),
  package clippy (0), `cargo fmt --all --check` (0), `check-cli-reference.py --check` (0),
  `check-provenance.sh` (0), `coverage-report.py --check` (0), and the `no-default-features` rows
  for all three packages.
