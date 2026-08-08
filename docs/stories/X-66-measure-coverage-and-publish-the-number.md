---
id: X-66
title: Measure coverage and publish the number
pillar: Build
status: in-progress
priority: 6
design:
epic: conformance
areas: [scripts, ci]
predicate:
announcement:
note: 1756 test attributes and no measurement of what they reach · a number that is generated, never asserted · follow-up
---

# Measure coverage and publish the number

## Goal

Generate a coverage figure from the suite rather than having none, so the question "what does this
suite not reach" has an answer that does not depend on somebody reading 79 test files.

## Acceptance

- [x] A CI job measures workspace line and branch coverage with `cargo llvm-cov` and publishes the
      result as a build artifact.
- [x] The figure is **generated into the docs, never transcribed** — the same rule
      `docs/compliance.md` and `docs/maturity.md` already follow. A hand-written percentage anywhere
      in the tree fails this story.
- [x] The job is registered in `scripts/gate.py`, either as a step or in `NOT_RUN_LOCALLY` with a
      stated reason, so `gate.py --check` stays green and the gate cannot silently omit it.
- [x] The published figure states what it excludes (examples, fuzz targets, generated code) on the
      same surface that states the number.
- [x] **No coverage threshold gates the build in this story.** A ratchet is a separate decision; see
      Notes.
- [ ] `./scripts/gate.py` green.

## Progress
- 2026-08-08: implemented on `impl/X-66`.
  - `scripts/coverage-report.py` is the generator: `--measure` runs `cargo llvm-cov` and records
    *counts* in `docs/coverage/measurement.json`; the default mode renders `docs/coverage.md` from
    them; `--check` byte-compares the page against that rendering. **No percentage is ever stored** —
    the schema rejects a `percent` key on any counter, so every figure on the page is arithmetic
    performed at render time. That is the 941-tests scar closed for this measurement.
  - The `coverage` CI job installs `cargo-llvm-cov --locked` (the repository's pinning convention,
    no version), measures on `nightly` because `--branch` is unstable in the tool, appends the
    figure to the run summary and uploads the JSON, lcov and browsable HTML as a build artifact.
  - `gate.py`: `coverage` is in `NOT_RUN_LOCALLY` with a reason — an instrumented rebuild plus a
    second full run of the suite. The cheap half is two new local steps, `coverage report`
    (`docs` job) and `coverage report tests` (`gate` job). `gate.py --check` reports 39 steps over
    21 CI jobs, none unaccounted for.
  - **Nothing gates on the number, and the suite is what holds that.** A measurement covering
    nothing checks green; `--fail-under-lines/-functions/-regions` is asserted absent from the
    checker, from the measurement command and from the CI job.
  - First recorded measurement, at `fc4fe49`: lines 90.13% of 74818, branches 69.77% of 8365,
    functions 91.72% of 8126. The page carries the four limits that number does not cover, the
    largest being that inline `#[cfg(test)] mod tests` lives in `src/` and is counted — path
    exclusions cannot reach it, so the line figure is flattered by however much unit-test code sits
    beside the code it tests. Read the per-crate `Lines unreached` column rather than the headline.
  - `docs/comparison.md` regenerated: its generated gate-step cell moved 37 → 39. Derived artifact,
    not typed — `AGENTS.md` requires it in the same change.
  - Deliberately **not** done, and left as separate decisions: a staleness expiry on the recorded
    measurement (a red gate on a date with no code change behind it), a ratchet, and any per-crate
    threshold.

- 2026-08-08: **readiness audit — ready as written**, with one constraint the implementor must not
  violate: no coverage tool is pinned anywhere in the repo today, and `docs/roadmap.md` explicitly
  refuses a v1 gate built on coverage, while the generated region of `docs/maturity.md` already says
  "Nothing here measures whether the tests are good, only that they pass." The number is generated
  and published; it is never asserted, never gated on, and never presented as a quality claim.

## Notes
- Found by the 2026-08-04 capability review: there is no `cargo llvm-cov` or tarpaulin step anywhere
  in the gate or CI, and `docs/maturity.md` already names the deeper limit honestly — "nothing here
  measures whether the tests are good, only that they pass". Coverage does not fix that; it bounds it.
- Deliberately no threshold. A coverage gate rewards tests written to touch lines, which is the
  `X-36` failure shape in a new place: it looks like coverage and is not. Measure first, decide about
  a ratchet later with the number in hand.
- Follow-up, not beta-1: it changes nothing a user can observe and blocks no announcement predicate.
