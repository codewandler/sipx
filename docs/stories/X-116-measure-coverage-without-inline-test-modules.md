---
id: X-116
title: Measure coverage without counting inline test modules
pillar: Build
status: ready
priority: 26
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
- [ ] The mechanism is stated on the page, and its cost is stated with it: whether it needs
      `#[coverage(off)]`, a cfg the crates do not yet carry, or a tool upgrade.
- [ ] The exclusion is generated from a rule, not a hand-maintained file list.
- [ ] Nothing gates on the resulting number, exactly as `X-66` established.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `X-66`'s adjacent findings. `--ignore-filename-regex` cannot reach a test
  module inside a source file, and this project keeps unit tests inline, so the headline 90.13% is
  high for a reason unrelated to how well the code is tested. `X-66` states this first among its
  limits and points readers at the per-crate unreached-lines column instead.

## Notes

- The number worth acting on is branch coverage — 69.77% overall, 62-64% in `sipx-app`, `sipx-call`,
  `sipx-cli` and `sipx-ua` — and it is on the least stable footing, being unstable in the tool.
- This story has a real cost/benefit argument to make; a conclusion of "not worth it, and here is
  why" is an acceptable outcome if the page then says so.
