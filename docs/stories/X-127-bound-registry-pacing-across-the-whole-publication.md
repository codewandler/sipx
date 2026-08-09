---
id: X-127
title: Bound registry pacing across the whole publication, not one invocation
pillar: Build
status: in-progress
priority: 42
design: docs/specs/release-rehearsal.md
epic: conformance
areas: [release, ci]
predicate:
announcement:
note: found while verifying X-119 against X-93's rate-limit row · the per-run budget resets
---

# Bound registry pacing across the whole publication, not one invocation

## Goal

Make the registry rate-limit budget a bound on a *release* rather than on one helper invocation, and
close the two places where `release.py`'s pacing behaviour and its specification disagree, so that a
first multi-crate publication has a finite total cost a reviewer can read off the workflow.

## Acceptance

- [x] `--registry-retry-budget-seconds` is passed explicitly by both `.github/workflows/crates-io.yml`
      and `.github/workflows/crates-io-resume.yml`, so the bound a protected release runs under is
      visible in the file that runs it rather than left at a helper default.
- [x] The frontier loop's total pacing cost is bounded across its `public_count + 1` invocations, not
      only within each one. Today each rerun restarts the `1800`-second budget, so the only bound over
      a whole publication is the job's `timeout-minutes`.
- [x] `docs/specs/release-rehearsal.md` and `scripts/release.py` agree on the unreadable name probe.
      The spec states it "is a registry failure and refuses the invocation, exactly as the version
      probe does"; `_registry_name_exists` returns `None` and `main` prints
      `the registry gave no readable name answer; pacing it under the stated new-crate limit` and
      continues. One of the two is wrong; a failing-first test names which.
- [x] A test covers `main`'s own `new_crates` derivation: that an existing name read from
      `_registry_name_exists` yields an empty new-crate set, and that an unreadable answer paces the
      name conservatively. `X-93` proved both bucket behaviours at the `publish_frontier` boundary
      with a hand-supplied `new_crates`; nothing exercises the loop in `main` that computes it.
- [x] `NEW_CRATE_RATE_LIMIT` and `NEW_VERSION_RATE_LIMIT` say where their values come from and what
      would make them stale. They are crates.io's published policy transcribed as constants, with no
      check that the policy still says that.
- [ ] `./scripts/gate.py` is green.

## Progress
- 2026-08-08: filed from the `X-93` implementation. `X-119` satisfies `X-93`'s rate-limit row for
  registry-provided deadlines, checksum-proven resume and the two-bucket model; `X-93` added the
  repeated-429 and mixed-frontier tests it was missing. Everything above is what remains, and none
  of it fits inside either story's Acceptance.

## Notes
- Evidence: `scripts/release.py:112-118` (the transcribed constants and the attempt bound),
  `:1290-1341` (`publish_frontier`'s per-invocation `remaining`), `:1130-1156` (`_registry_name_exists`
  returning `None`), `:2120-2141` (`main`'s untested `new_crates` derivation),
  `docs/specs/release-rehearsal.md:213-215` (the contradicted sentence).
- `X-119` records the missing workflow argument as left open in its own `## Progress`.

- 2026-08-08: renumbered from `X-126` on filing. `X-126` was allocated in the same wave, to the
  load-pair test that rejects a call under suite contention. Only the id moved.

- 2026-08-09: implemented on `impl/X-127`. Five of the six rows are ticked; `gate` is left for the
  integrating run.

  **Where the whole-publication budget lives.** `--registry-retry-ledger PATH` — a JSON file holding
  one key, `spent_seconds`, and nothing else. `main` reads it into `publish_frontier`'s new
  `spent_seconds`, and `record_wait` charges each wait to it *before* the wait is served. Both
  workflows create the ledger once, outside the frontier loop (`$RUNNER_TEMP/sipx-release-pacing.json`
  and `.../sipx-recovery-pacing.json`), so `public_count + 1` invocations share one budget.

  **Why it cannot lose a published crate.** The ledger records elapsed seconds, never what was
  published. Every invocation still derives its frontier from `_registry_available` and
  `verify_resume_bytes`, so a ledger that is lost, copied or discarded changes only how long a run
  may wait — it can neither skip nor repeat an upload. Charging before the pause means a run killed
  mid-wait overstates the bound rather than losing it, and overstating can only stop a later
  invocation earlier, which is the resumable outcome. An exhausted budget still raises *before*
  dispatching the next upload, which is `X-119`'s property unchanged.
  `test_the_pacing_ledger_carries_a_publications_spend_between_invocations` drives three `main`
  invocations over one ledger and asserts the crate published by the first is re-proved against the
  registry's checksums and skipped, not republished.

  **The spec/code disagreement, settled in favour of the code.** An unreadable name answer paces
  conservatively and does not refuse. A name answer only selects *which* stated allowance paces an
  upload and can neither skip nor repeat one, and a wrong guess is corrected by the registry's own
  `429`; a version answer decides what is published, so it still refuses. `docs/specs/release-rehearsal.md`
  §4.1 now says that, `R16` is narrowed to the exact-version probe, and `R24` is the name probe's own
  vector. `test_the_specification_and_the_helper_agree_on_an_unreadable_name_answer` holds both ends.

  **Budgets named in the workflows.** `4800` for `crates-io.yml` (twelve public crates at a burst of
  five then one every `600`s is `4200`s of unavoidable waiting, inside a 180-minute job that also
  runs the gate) and `7200` for `crates-io-resume.yml` (same bound, no gate in that job). Exhaustion
  now hands over to the resume workflow instead of to `timeout-minutes`.

  **Failing-first evidence**, all at `465605dd4cd04c5fd36ee32913f7d323efa77d22`:
  `python3 scripts/test-release.py TheRegistryRateLimit` → `FAILED (failures=3, errors=3)`;
  `publish_frontier() got an unexpected keyword argument 'spent_seconds'`,
  `module 'sipx_release' has no attribute 'pacing_ledger_spent'`,
  `'refuses the invocation, exactly as the version probe does' unexpectedly found`.
  `python3 scripts/test-release-workflow.py` → `publication does not name a finite rate-limit budget`
  and `frontier loop does not carry one rate-limit budget across its invocations`.

  **Owed CHANGELOG sentence** (the coordinator owns that file): *Registry publication now spends one
  rate-limit budget across a whole release rather than restarting it on every frontier rerun, and
  both publication workflows state that budget themselves.*

  **Left open**, filed as `X-140`: the two token *buckets* still reset on every invocation, so the
  model believes it may burst five new names per rerun and only the registry's `429` corrects it.
  The ledger bounds the resulting waiting, so the cost is bounded refusals rather than lost uploads.
  The board (`docs/stories/README.md`) is fenced for this implementor and needs regenerating for
  both `X-127` and the new `X-140`.
