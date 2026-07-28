---
id: A-6
title: Finish the engine-binding spec — isolation, lifecycle, budgets
pillar: Application
status: backlog
priority:
design: docs/designs/embedded-runtime.md
epic: app-host
areas: [sipx-app]
note: app-host phase 3 · spec before code, decided with measurements where the design says so
---

# Finish the engine-binding spec — isolation, lifecycle, budgets

## Goal
Close [specs/engine-binding.md](../specs/engine-binding.md) §3: isolate granularity and
pooling, handler load/reload, CPU and heap budgets and their mapped outcomes, transpile
caching — each with vectors, the granularity choice justified by measurement.

## Acceptance
- [ ] Every §3 open point closed; the parity-suite vector set defined well enough that `A-5`
      derives tests from it.
- [ ] The isolate-granularity decision is recorded with the measurement that made it, under
      the constraint the spec already fixes (one failure, one call).
- [ ] The sandbox-honesty statement (capability isolation, not an OS boundary; session mode
      for untrusted code) appears in the spec and is earmarked for the public docs.

## Progress
- Not started.

## Notes
- Engine hosting is `sipx-app`'s concern alone; the protocol core stays engine-free — the
  [app-host](../designs/app-host.md) ground rules make that structural.
