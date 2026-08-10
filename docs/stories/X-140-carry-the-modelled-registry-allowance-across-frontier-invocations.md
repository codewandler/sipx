---
id: X-140
title: Carry the modelled registry allowance across frontier invocations, not just its cost
pillar: Build
status: ready
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

- [ ] `publish_frontier`'s two token buckets are seeded from what earlier invocations of the same
      publication already spent, not from `rate_limit_bucket(limit, monotonic())`. Today
      `X-127`'s ledger carries the *cost* of pacing across invocations while the *model* resets, so
      invocation two believes it may upload five new names immediately, and only the registry's own
      `429` says otherwise.
- [ ] The carried state uses a clock that is comparable between processes. `monotonic()` is not:
      the buckets are keyed on it today, and a bucket serialised from one process is meaningless in
      the next. Whatever replaces it must keep `X-119`'s property that the registry's stated
      deadline overrides the model.
- [ ] A failing-first test with an injected clock shows a second invocation waiting for an
      allowance the first one spent, where today it dispatches immediately and is refused.
- [ ] No published crate is lost or republished by the change: the frontier still comes from
      registry visibility and checksum-proven bytes, and a lost or unreadable carried model degrades
      to the current optimistic start rather than to a skipped upload.
- [ ] `./scripts/gate.py` is green.

## Notes
- Evidence: `scripts/release.py:1350-1356` (both buckets seeded at `monotonic()` on every
  invocation), `docs/specs/release-rehearsal.md` §4.1 (states the budget spans a publication and is
  silent on the model), `.github/workflows/crates-io.yml` (thirteen invocations for twelve public
  crates).
- Cost of not doing it is bounded, which is why this is separate from `X-127`: the wasted work is
  refusals rather than uploads, `rate_limit_restate` corrects the model on the first one, and the
  pacing ledger bounds the total waiting either way. What it buys is a publication that spends its
  budget on waiting rather than on being told to wait.
