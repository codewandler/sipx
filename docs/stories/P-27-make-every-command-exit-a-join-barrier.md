---
id: P-27
title: Make every command exit a join barrier
pillar: Phone
status: done
priority:
design:
epic: diagnostic-automation
areas: [sipx-cli, sipx-ua]
predicate:
announcement:
note: P-25 added the join on register's deadline path only · every other exit still reports before its work is observably finished
---

# Make every command exit a join barrier

## Goal

Make the terminal record of every diagnostic command mean the same thing it means on `dial`: that
the work is observably finished, not merely that the result is known.

## Acceptance

- [x] `register` joins its endpoint on every exit — success, rejection and transport failure — not
      only on the deadline path `P-25` added.
- [x] A failing-first test proves no socket, task or timer outlives the terminal record, for each
      exit class of each long-running command.
- [x] `--keep-alive` stops sending a redundant second REGISTER per invocation: it currently calls
      `register_candidates` and then `keep_registered()`, which registers again immediately.
- [x] `--keep-alive` refreshes are bounded rather than governed only by the granted lease, matching
      the deadline `P-25` established for the initial attempt.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `P-25`'s adjacent findings. That story added `handle.shutdown()` on the
  timeout path because its acceptance asked for it, and deliberately left the rest: `register` never
  joins on a non-timeout failure or on success, so its counters are read from a still-running
  endpoint.

- 2026-08-08: **implemented** on `impl/P-27`, off `wave/rc6`.

  `register`'s exits now all pass through one of two barriers. `report_joined` in
  `crates/sipx-cli/src/register.rs` shuts the endpoint down and *then* exports the counters and
  emits, and `report_failure` — which every rejection, transport failure and keep-alive failure
  reaches — does the same on stderr, so a failure carries the run's final counters in its report
  instead of leaving them to `Export`'s destructor. `report_attempt_timeout` is unchanged; it was
  already the shape. A record that is not the last one is deliberately exempt: under `--wake` or
  `--keep-alive` the registration line is progress and the barrier moves to whatever ends the run,
  because the endpoint is still needed for what follows. `report_wake` became `wake_report`, which
  builds the record and lets the caller decide whether it is terminal.

  `--keep-alive` no longer registers twice. `UserAgent::keep_registered_from(lease, budget)` in
  `crates/sipx-ua/src/agent.rs` continues from a lease the caller already holds;
  `keep_registered` is now that method after one `register`, so its behaviour is unchanged. The
  `budget` bounds every refresh through `register_within`, which is the deadline `P-25` gave the
  initial attempt applied to each attempt after it — `None` keeps the transaction layer's own
  schedule, which is what `--timeout 0` asks for.

  Beyond `register`, three commands had exits that reported before joining: `load` never shut its
  endpoint down at all, `answer`'s `refuse` emitted its record *before* the shutdown and had four
  unjoined internal-failure exits, and `dial` had one. All are joined now. `docs/specs/
  diagnostic-phone.md` §3.5 states the barrier normatively for every exit and restates
  `--keep-alive`; `website/docs/reference/cli.md` says the same for an operator.

  Failing-first: `register::tests::every_exit_joins_the_endpoint_before_the_terminal_record`
  (success, rejection, transport failure, deadline) failed at the merge base with *"the signalling
  endpoint still holds 127.0.0.1:33938 after the terminal record (Address already in use)"*, and
  `keep_alive_registers_once_per_invocation` and
  `keep_alive_refreshes_are_bounded_by_the_stated_deadline` in `crates/sipx-cli/tests/cli.rs`
  failed with a second REGISTER at `CSeq: 2` and with the 32-second transaction schedule
  respectively. The probe is `crates/sipx-cli/src/join_probe.rs`: rebinding the command's own
  local address with no `.await` in between, which decides both answers before it runs.

  Left honest rather than overstated: `load`'s probe
  (`load::tests::the_summary_joins_the_endpoint_before_it_is_printed`) **passes at the merge base
  too**, because dropping the last handle and then awaiting the signal task had already released
  the socket. It is a regression guard, not a failing-first test; what changed for `load` is that
  the shutdown is now ordered and waits on the endpoint's cleanup barrier. `answer`'s and `dial`'s
  repaired exits have no dedicated test — each needs a full call plus an induced internal failure
  — and are covered only by the existing suites staying green.

- 2026-08-08: closed in the `1.0.0-rc.6` boundary.

## Notes

- `P-25`'s `report_attempt_timeout` is the shape to follow, and `dial` already does this correctly.
- Reading counters after `shutdown()` is deliberate there; keep that ordering.
