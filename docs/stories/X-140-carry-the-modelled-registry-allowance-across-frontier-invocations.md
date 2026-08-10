---
id: X-140
title: Carry the modelled registry allowance across frontier invocations, not just its cost
pillar: Build
status: in-progress
priority: 9
design: docs/specs/release-rehearsal.md
epic: conformance
areas: [release, ci]
predicate:
announcement:
note: found while implementing X-127 · the budget now spans invocations, the token buckets do not
---

# Carry the modelled registry allowance across frontier invocations, not just its cost

## Goal

Stop each frontier invocation from starting with a full burst of registry tokens it has not earned,
so a first multi-crate publication provokes the refusals it can predict instead of discovering them.

## Acceptance

- [x] `publish_frontier`'s two token buckets are seeded from what earlier invocations of the same
      publication already spent, not from `rate_limit_bucket(limit, monotonic())`. Today
      `X-127`'s ledger carries the *cost* of pacing across invocations while the *model* resets, so
      invocation two believes it may upload five new names immediately, and only the registry's own
      `429` says otherwise.
- [x] The carried state uses a clock that is comparable between processes. `monotonic()` is not:
      the buckets are keyed on it today, and a bucket serialised from one process is meaningless in
      the next. Whatever replaces it must keep `X-119`'s property that the registry's stated
      deadline overrides the model.
- [x] A failing-first test with an injected clock shows a second invocation waiting for an
      allowance the first one spent, where today it dispatches immediately and is refused.
- [x] No published crate is lost or republished by the change: the frontier still comes from
      registry visibility and checksum-proven bytes, and a lost or unreadable carried model degrades
      to the current optimistic start rather than to a skipped upload.
- [x] `./scripts/gate.py` is green.

## Notes
- Evidence: `scripts/release.py:1350-1356` (both buckets seeded at `monotonic()` on every
  invocation), `docs/specs/release-rehearsal.md` §4.1 (states the budget spans a publication and is
  silent on the model), `.github/workflows/crates-io.yml` (thirteen invocations for twelve public
  crates).
- Cost of not doing it is bounded, which is why this is separate from `X-127`: the wasted work is
  refusals rather than uploads, `rate_limit_restate` corrects the model on the first one, and the
  pacing ledger bounds the total waiting either way. What it buys is a publication that spends its
  budget on waiting rather than on being told to wait.

## Progress

- 2026-08-10: specified and implemented a sibling `<ledger>.allowances` model rather than adding
  allowance state to `X-127`'s strict spend ledger. The spend record must refuse when unreadable or
  the whole-publication bound disappears; the model is only pacing advice and must instead degrade
  to the optimistic bursts. Keeping the records separate gives both facts their required failure
  mode while one CLI ledger path still anchors the frontier loop.
- Failing first: `python3 scripts/test-release.py
  TheRegistryRateLimit.test_a_second_invocation_waits_for_the_new_name_allowance_the_first_spent
  TheRegistryRateLimit.test_a_registry_deadline_replaces_and_crosses_the_persisted_model
  TheRegistryRateLimit.test_a_missing_or_unreadable_allowance_model_degrades_to_an_optimistic_start`
  ran three tests and raised three errors because `pacing_allowance_model` and
  `pacing_allowance_model_path` did not exist. The injected-clock vector represented two processes
  at the same Unix instant; without carried state the second would dispatch before the 600-second
  refill and receive the fixture `429`.
- Both new-name and existing-name buckets now persist their level, update instant and registry
  `not_before` instant on a Unix clock. An attempted upload is charged before dispatch; a `429`
  restatement is persisted immediately and therefore still replaces the model across processes.
  Atomic model writes that fail, and absent, malformed or unreadable model files, are reported and
  fall back to full stated bursts. Neither record contains package names; readiness and skip/resume
  remain derived from registry visibility and checksum-proven bytes.
- Focused verification: all 23 `TheRegistryRateLimit` tests pass; all 84 release rehearsal tests
  pass, including the clean packaged-consumer proof; all 56 release-workflow tests pass; and
  `check-release-workflow.py --check`, `check-fixed-sleep.py --check`, the provenance check, and the
  documentation-link check pass.
- The complete local acceptance gate passed all 51 steps on the rc.23 candidate on 2026-08-10.
