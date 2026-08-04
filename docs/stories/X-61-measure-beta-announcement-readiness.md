---
id: X-61
title: Measure hypothetical public beta announcement readiness
pillar: Build
status: done
priority: 8
design: docs/roadmap.md
epic: commit-snapshot
areas: [release, docs]
predicate: 7
announcement:
note: generate five all-or-nothing beta predicates without turning RFC coverage into a score
---

# Measure hypothetical public beta announcement readiness

## Goal

Make the hypothetical readiness for broader `1.0.0-beta.1` publicity as explicit and drift-resistant
as the existing alpha measurement, while keeping the stable v1 gate and authorization separate.

## Acceptance

- [x] `docs/roadmap.md` defines five beta-announcement predicates: integrity, product proof,
      claimed interop, registry distribution and an honest adoption surface.
- [x] Stories declare those predicates through `announcement:` frontmatter; an invalid number fails
      and a computed predicate no story declares reports unknown.
- [x] Integrity is derived from all seven alpha predicates rather than from a second blocker list.
- [x] `docs/maturity.md` generates the state and waiting stories for both gates without an aggregate
      RFC or weighted maturity percentage.
- [x] Failing-first generator tests cover an open announcement story, an invalid declaration, an
      undeclared predicate and alpha-derived integrity reopening.

## Progress

- Complete. The roadmap, story schema and generated maturity report now carry one all-or-nothing
  hypothetical beta-announcement threshold. The full gate exercises the reversed predicate cases
  and rejects a stale report; meeting it authorizes no publicity.

## Notes

- `1.0.0-beta.1` permits documented breaking changes. Only `1.0.0` freezes supported APIs.
- This story changes measurement and roadmap state, not the runtime product.
