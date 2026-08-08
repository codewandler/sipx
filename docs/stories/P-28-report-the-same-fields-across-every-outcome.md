---
id: P-28
title: Report the same fields across every outcome
pillar: Phone
status: in-progress
priority: 6
design:
epic: diagnostic-automation
areas: [sipx-cli, scripts]
predicate:
announcement:
note: register's success report carries aor and its failure report does not, so no script can match on it across both
---

# Report the same fields across every outcome

## Goal

Make a structured result answer the same questions whichever way a command ended, so automation can
read one field without branching on success first.

## Acceptance

- [x] Every command's result carries its identifying fields on all outcomes — `register`'s `aor` is
      the known case, present on success and, since `P-25`, on timeout, but absent on rejection and
      transport failure.
- [x] A repository check derives the field set per command from the code and fails when an outcome
      omits a field a sibling outcome carries, so this cannot regress silently.
- [x] A failing-first test covers at least one command across all of its outcome classes.
- [ ] No field is added to a published schema without a `CHANGELOG.md` entry; the JSON contract
      table stays the source of truth.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `P-25`'s adjacent findings. It added `aor` to the timeout report only, to
  keep its diff scoped, and recorded the inconsistency rather than widening silently.

- 2026-08-08: **PARTIAL.** `register`'s failure report now carries `aor` on every outcome, not only
  success and timeout, proved failing-first by removing the field and watching
  `every_register_outcome_names_its_address_of_record` fail. That is the case `P-25` recorded and the
  one a scheduled check actually reads.
  The **repository check** row is not done: deriving each command's field set from the code and
  failing when one outcome omits a field a sibling carries is the story's real weight, and it needs
  a parser over the report builders rather than a per-command fix. Left rather than half-built —
  without it this is one instance fixed, not the class.

- 2026-08-08: **the repository check ships.** `scripts/check-outcome-parity.py` reads every
  `Report::new()` builder chain under `crates/sipx-cli/src/`, groups the chains by the module that
  is their command, and fails when one outcome names a field a sibling does not. Gate step
  `outcome parity` in the `docs` cluster, suite `outcome parity tests` in the `gate` cluster, both
  mirrored in `ci.yml`; `./scripts/gate.py --check` reports 44 steps over 22 CI jobs with none
  unaccounted for.

  **Proved failing-first, in both directions.** Adding `.text("registrar_host", …)` to `register`'s
  success chain alone:

  ```
  register: `registrar_host` is reported by `registered` and not by `register.rs:492`, `timeout`
  and `woken`; a script reading it has to branch on the outcome first. Report it there too, or
  declare it in OUTCOME_SPECIFIC with why it cannot be.
  register reports `registrar_host` and website/docs/reference/cli.md does not name it; a consumer
  would have to read the source to find out it exists
  ```

  And removing `aor` from the failure record — literally the `P-25` state this story was filed
  from — reproduces the original finding:

  ```
  register: `aor` is reported by `registered`, `timeout` and `woken` and not by `register.rs:491`;
  a script reading it has to branch on the outcome first.
  ```

  **It found a second instance of the class.** `dial` carried `peer` on the answered record and on
  neither the refusal/transport-failure record nor the interrupt one — `register`'s defect in the
  sibling command, which nobody had noticed. Fixed, and proved failing-first through the shipped
  process by `every_dial_outcome_names_the_peer_it_called`, which saw
  `{"status":"busy","error":"rejected: 486 Busy Here"}` before the change. `answer` reports
  `caller` on both of its comparable outcomes already.

  **What the checker can and cannot see, because a check that reports less than it claims is worse
  than none.** It compares the fields a chain names *itself*. It cannot see a field added through a
  binding (`report = report.boolean("flow", …)`), a field contributed by a helper the report is
  passed to (`transport.report(…)`, `with_attempts(…)`, `export.into_report(…)`), or a field whose
  name is not a string literal. That blind spot is 66 field additions today; the checker counts
  them in its success line and `--explain` names each with its file and line, so the limit is
  visible in the gate's output rather than only in a comment. It also refuses to report at all if
  it finds no command with sibling outcomes, or fewer than its plausibility floors — the failure
  mode of `X-117`, `X-38` and `X-120` was reporting emptiness as success.

  Today it compares 9 outcomes across `answer`, `dial` and `register`; `peers` has one outcome and
  is therefore not evidence of parity, `counters` builds a fragment with no `status`, and two of
  `answer`'s records are declared in `WITHOUT_A_CALL` because they are reached before any INVITE
  has arrived. Every exemption is a named entry with a reason, and an exemption nothing needs any
  more is itself a finding.

  **The fourth row is half done and stays unticked.** `documentation_problems` now requires every
  field a command reports to be named on `website/docs/reference/cli.md`, which is the "contract
  table stays the source of truth" half and which caught `answer`'s undocumented `code` on its
  first run. The `CHANGELOG.md` half is not mechanized: whether a field addition is significant
  enough to be announced is a judgement, and `CHANGELOG.md` is reconciled outside this branch. The
  entry this change needs is:

  > `sipx dial` reports `peer` on every outcome, not only on an answered call — a refusal, a
  > transport failure and an interrupt all name the URI that was dialled, so a script can attach
  > any record to the call it placed without branching on success first.
  > `scripts/check-outcome-parity.py` derives each command's field set from its report builders and
  > fails when one outcome omits a field a sibling carries.

  `./scripts/gate.py` was deliberately not run on this branch (one gate per wave); verified here
  with the checker, its 39-test suite, `./scripts/gate.py --check`, `check-provenance.sh`,
  `check-story-closure.py`, `maturity.py --check`, `cargo test -p sipx-cli --all-features`, clippy
  and `cargo fmt --all`. `docs/stories/README.md` needs regenerating for this status change.

## Notes

- `P-21` made repeated fields unrepresentable; this is the complementary gap — fields that are
  absent rather than duplicated.
