---
id: X-127
title: Bound registry pacing across the whole publication, not one invocation
pillar: Build
status: backlog
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

- [ ] `--registry-retry-budget-seconds` is passed explicitly by both `.github/workflows/crates-io.yml`
      and `.github/workflows/crates-io-resume.yml`, so the bound a protected release runs under is
      visible in the file that runs it rather than left at a helper default.
- [ ] The frontier loop's total pacing cost is bounded across its `public_count + 1` invocations, not
      only within each one. Today each rerun restarts the `1800`-second budget, so the only bound over
      a whole publication is the job's `timeout-minutes`.
- [ ] `docs/specs/release-rehearsal.md` and `scripts/release.py` agree on the unreadable name probe.
      The spec states it "is a registry failure and refuses the invocation, exactly as the version
      probe does"; `_registry_name_exists` returns `None` and `main` prints
      `the registry gave no readable name answer; pacing it under the stated new-crate limit` and
      continues. One of the two is wrong; a failing-first test names which.
- [ ] A test covers `main`'s own `new_crates` derivation: that an existing name read from
      `_registry_name_exists` yields an empty new-crate set, and that an unreadable answer paces the
      name conservatively. `X-93` proved both bucket behaviours at the `publish_frontier` boundary
      with a hand-supplied `new_crates`; nothing exercises the loop in `main` that computes it.
- [ ] `NEW_CRATE_RATE_LIMIT` and `NEW_VERSION_RATE_LIMIT` say where their values come from and what
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
