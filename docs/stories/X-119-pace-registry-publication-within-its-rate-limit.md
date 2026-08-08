---
id: X-119
title: Pace registry publication within its rate limit
pillar: Build
status: in-progress
priority: 32
design:
epic: conformance
areas: [scripts, release]
predicate:
announcement:
note: split out of X-93 · the 429 pacing row shares nothing with the rest of that story
---

# Pace registry publication within its rate limit

## Goal

Publish the crate set without tripping the registry's rate limit, and without a fixed sleep standing
in for knowing the limit.

## Acceptance

- [x] `scripts/release.py` paces publication against the registry's stated limit and its response
      headers, rather than a constant delay between crates.
- [x] A `429` is retried within a bounded budget and reported as itself; exhausting the budget stops
      before any further publication rather than continuing.
- [x] A failing-first test drives the pacing from injected responses, including a `429` with and
      without a retry hint, with no wall-clock sleep in the test.
- [x] Publication remains resumable: a paced run that stops mid-frontier restarts without
      republishing or moving anything already published.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: split out of `X-93` per the rc.4 readiness audit, which found this row shares nothing
  with that story's other four and is story-sized on its own.

- 2026-08-08: **implemented.** One correction to the story's premise, found by reading the helper
  before changing it: at `b66d230` publication had **no** delay between crates, constant or
  otherwise — `main` ran the frontier's `cargo publish` commands back to back. So this replaces
  *nothing* with the registry's own limit rather than replacing a placeholder constant, and the
  Goal's "without a fixed sleep standing in for knowing the limit" is met by never introducing one.

  `docs/specs/release-rehearsal.md` §4.1 is the new authority, with vectors `R21`–`R23`.

  **How the pacing learns the limit.** crates.io states two allowances that differ by an order of
  magnitude — a new crate name once per ten minutes after a burst of five, a new version of an
  existing name once per minute after a burst of thirty. Each is modelled as a token bucket
  (`NEW_CRATE_RATE_LIMIT`, `NEW_VERSION_RATE_LIMIT`), and `_registry_name_exists` selects one per
  package so an ordinary version bump is never paced as first-name creation. When the registry
  answers `429`, it and not the model is the authority: `rate_limit_refusal` reads the deadline from
  a `Retry-After` header (delta-seconds or HTTP-date) or from the refusal body's "try again after"
  timestamp, and `rate_limit_restate` makes that deadline the moment one upload is permitted again,
  pacing the rest of that class from it.

  **What it falls back to when the registry says nothing.** A `429` carrying no hint falls back to
  that class's *stated* refill interval — ten minutes for a new name, one minute for a new version —
  not to an invented constant. An unreadable name probe paces under the new-crate allowance rather
  than refusing the release, because a pacing hint can neither skip nor repeat an upload; the exact
  version probe still governs what is published and keeps its fail-closed strictness.

  **The retry budget.** `--registry-retry-budget-seconds`, default `1800`, is the total wall clock
  one invocation may spend waiting on rate limits, plus `RATE_LIMIT_RETRY_ATTEMPTS = 3` per package.
  1800 s is three consecutive new-crate windows and matches the existing `--command-timeout-seconds`
  default; it sits well inside the release job's 180-minute bound. Three attempts covers a deadline
  read, waited out, and restated once — beyond that the registry is refusing rather than pacing.
  Either exhaustion raises before the next `cargo publish` is dispatched, quotes the registry's own
  `429` line, and names what this run did publish.

  **Failing-first proof**, at merge base `b66d230` with `scripts/release.py` untouched:

      $ python3 scripts/test-release.py TheRegistryRateLimit
      AttributeError: module 'sipx_release' has no attribute 'publish_frontier'
      AttributeError: module 'sipx_release' has no attribute 'rate_limit_refusal'
      AttributeError: <module 'sipx_release'> does not have the attribute '_registry_name_exists'
      Ran 9 tests in 0.005s
      FAILED (errors=9)

  Every pacing test injects `monotonic`, `pause` and `now`; no test spends wall-clock time, and the
  end-to-end resume test stops the first run with a budget smaller than the stated deadline so it
  never sleeps either. `python3 scripts/test-release.py` is green (70 tests), as are
  `./scripts/check-fixed-sleep.py --check`, `./scripts/gate.py --check`,
  `./scripts/check-release-workflow.py --check`, `./scripts/check-docs-links.py` and
  `./scripts/check-provenance.sh`. The full `./scripts/gate.py` was deliberately not run here; it
  belongs to the wave's single gate run, so the last Acceptance row stays unticked.

  Left open for the coordinator: `.github/workflows/crates-io.yml` and `crates-io-resume.yml` do not
  pass `--registry-retry-budget-seconds`, so both take the 1800 s default. That is finite and within
  the job bound, but naming it in the workflow would make the release's total wait reviewable.
