---
id: X-152
title: Record private downstream adoption without disclosing the product
pillar: Build
status: done
priority: 1
design: docs/roadmap.md
epic: v1-release
areas: [release, docs, api]
predicate:
announcement:
promotion: 3
note: stable predicate 3 · separate-product evidence may remain confidential, but the public attestation must say exactly what was verified
---

# Record private downstream adoption without disclosing the product

## Goal

Record the real adoption of sipx by a separately governed private downstream product without
publishing that product's identity, repository, infrastructure or internal revision. Replace the
roadmap's ambiguous authorship test with a product-boundary test that distinguishes a real
downstream integration from an example manufactured inside this repository.

## Acceptance

- [x] `docs/roadmap.md` says that a public or private application counts when it is developed in a
      separate repository under its own product requirements, consumes an exact public sipx source,
      and exercises a non-test production path. A local path dependency, vendored copy, fork,
      workspace example or consumer written only to satisfy the predicate does not count.
- [x] The public record does not claim unrelated authorship. It permits overlapping contributors
      while requiring independent product ownership, requirements and release decisions.
- [x] This story records the privacy-safe result of the evidence actually reviewed: exact public
      sipx input, a non-test production dependency, reproducible validation, and whether a
      Supported boundary broke. It contains no private product name, path, repository URL, source
      revision, build or CI metadata, architecture detail, authorship metadata or infrastructure
      identifier.
- [x] `scripts/maturity.py` generates all five stable evidence predicates from checked state. Alpha
      continuity and per-layer caller reachability are derived; the remaining associations live in
      `promotion:` story frontmatter, so deleting or mistyping the adoption record cannot manufacture
      stable readiness.
- [x] Failing-first tests cover an open adoption story, a missing declaration, an invalid stable
      predicate number, reopened alpha integrity, missing post-alpha release evidence and the
      generated real-board stable table.
- [x] Focused maturity tests and checks, documentation links, provenance and whitespace checks pass.
- [x] The coordinator runs the complete gate after integration, then closes this story and
      regenerates the board and maturity report.

## Evidence reviewed

- The downstream pins public `v1.0.0-rc.23`, commit
  `004ac534b8b222060ad2d2308763efe6e1dedc10`, rather than a local path, private fork, vendored copy,
  workspace example or purpose-built adoption fixture.
- An authorized reviewer confirmed a committed non-test production integration in a separate
  private repository governed by that product's own requirements and release decisions. Focused
  validation was reproduced successfully. The private source, exercised architecture, build
  records, authorship and infrastructure are deliberately absent here.
- No incompatible Supported sipx API break was found at the pinned revision. Integration
  constraints were reviewed and retained privately rather than promoted into public product detail.

## Progress

- 2026-08-14: the seven stable-promotion tests failed first because `STABLE`, `PROMOTION_FIELD`,
  `stable_predicate_row` and the generated table did not exist. The completed suite has 67 passing
  cases. `maturity.py --check`, story closure, documentation links, provenance and `git diff
  --check` also pass in the implementation worktree.
- The generated table deliberately reads 4/5 while this story is `in-progress`. The coordinator's
  integrated full gate is the final unchecked row; after it passes, moving this story to `done` and
  regenerating the report changes predicate 3 to `met (recorded)` and the evidence total to 5/5.
- 2026-08-14: the integrated stable tree passed all 55 gate steps in 14m04s. The story is closed;
  the board and maturity report are regenerated as the final release-tree change.

## Notes

- Confidentiality changes what can be published, not what must be verified. The public attestation
  is reviewable for its criteria and exact sipx input; authorized reviewers can reproduce retained
  evidence without turning the private product into a public dependency of this repository.
- This story proves adoption of a public prerelease API. It does not claim that every Supported path
  was used, that the downstream is unrelated in authorship, or that downstream CI substitutes for
  sipx's stable release gate.
