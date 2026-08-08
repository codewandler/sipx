---
id: X-116
title: Measure coverage without counting inline test modules
pillar: Build
status: in-progress
priority: 28
design:
epic: conformance
areas: [scripts, ci]
predicate:
announcement:
note: the published 90% line figure is flattered because unit tests live inside src/ and path exclusion cannot reach them
---

# Measure coverage without counting inline test modules

## Goal

Make the published line-coverage figure describe the code under test rather than partly describing
the tests themselves.

## Acceptance

- [ ] `#[cfg(test)] mod tests` blocks inside `src/` are excluded from the measurement, and the
      published figure changes accordingly — a number that does not move proves the exclusion did
      not work.
      *Excluded: all 161 of them, and the exclusion is proven to move the number (see Progress).
      The published figure is not yet re-measured; `--measure` aborts on a merge-base test failure.*
- [ ] The mechanism is stated on the page, and its cost is stated with it: whether it needs
      `#[coverage(off)]`, a cfg the crates do not yet carry, or a tool upgrade.
      *Written and asserted in the generator; reaches `docs/coverage.md` with the same `--measure`.*
- [x] The exclusion is generated from a rule, not a hand-maintained file list.
      → `scripts/coverage-report.py:inline_test_modules` / `declaring_roots`, applied by
      `--annotate` and verified by `--check`; no path is named in any file.
- [x] Nothing gates on the resulting number, exactly as `X-66` established.
      → `test-coverage-report.py:NothingGatesOnTheNumber` still green, and
      `test_the_exclusion_gates_on_no_number` covers the new check.
- [ ] `./scripts/gate.py` green.
      *Not run — the wave coordinator runs one per wave. The `coverage report` step is red until the
      re-measurement, and `sipx-transport/tests/discards.rs` is red at the merge base.*

## Progress

- 2026-08-08: filed from `X-66`'s adjacent findings. `--ignore-filename-regex` cannot reach a test
  module inside a source file, and this project keeps unit tests inline, so the headline 90.13% is
  high for a reason unrelated to how well the code is tested. `X-66` states this first among its
  limits and points readers at the per-crate unreached-lines column instead.
- 2026-08-08: mechanism landed. `#[cfg_attr(coverage_nightly, coverage(off))]` on all 161 inline
  `#[cfg(test)] mod` items under `crates/*/src/`, and
  `#![cfg_attr(coverage_nightly, feature(coverage_attribute))]` in the 13 crate roots that head one
  — `sipx-host` heads none, and an unused `feature` gate is a warning only the measurement job would
  ever read. Both are generated: `coverage-report.py --annotate` applies one syntactic scan for
  `#[cfg(test)] mod`, `--check` fails on any module that escaped it, and no file is named anywhere.
  `Cargo.toml` declares `check-cfg` for `coverage`/`coverage_nightly` so the builds that never set
  the cfg do not warn about reading it.

  **It works, and here is the proof.** Same commit, same tool, one crate, the cfg the only
  difference (`cargo +nightly llvm-cov --package sipx-sdp --all-features --branch
  --ignore-filename-regex … --summary-only`, ± `--no-cfg-coverage-nightly`, 16s for the pair):

  | `sipx-sdp` | Lines counted | Lines | Functions | Regions | Branches |
  |---|---|---|---|---|---|
  | exclusion off | 3058 | 95.26% | 91.73% | 94.21% | 78.03% |
  | exclusion on | 1912 | **92.63%** | 89.33% | 90.58% | 78.03% |

  37.5% of the lines this crate's figure was computed over were its own tests. Branch coverage does
  not move at all — 264 branches either way — which is worth knowing given the Notes below: the
  number the story calls the one worth acting on is the one this story does not touch.

  **The published workspace figure is not refreshed.** `--measure` ran, built the instrumented
  workspace and then aborted (exit 101) because `sipx-transport/tests/discards.rs` fails — two
  discard sites in `sipx-call` (`event.rs:435`, `voice.rs:225`) have neither a counter nor a
  `// discard:` reason. That failure is at the merge base: the same command on `8684987` with the
  same two sites, exit 101, and this story's diff does not touch either file above its own test
  module. Nothing was recorded, by design. So `docs/coverage.md` still shows 90.13% measured at
  `fc4fe49`, and `coverage-report.py --check` is red saying exactly that — the record predates the
  exclusion. One `./scripts/coverage-report.py --measure` on a tree where that suite passes closes
  this story's remaining two Acceptance rows, and taking it on the integrated wave commit is where
  it belongs anyway.

## Notes

- The number worth acting on is branch coverage — 69.77% overall, 62-64% in `sipx-app`, `sipx-call`,
  `sipx-cli` and `sipx-ua` — and it is on the least stable footing, being unstable in the tool.
- This story has a real cost/benefit argument to make; a conclusion of "not worth it, and here is
  why" is an acceptable outcome if the page then says so.
