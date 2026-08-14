---
id: X-149
title: Refuse breaking changes to the v1 endpoint API
pillar: Build
status: done
priority: 5
design: docs/specs/api-compatibility.md
epic: v1-release
areas: [api, ci, release]
predicate:
announcement:
note: after A-41 · the supported surface needs a machine-readable release baseline before 1.0 freezes it
---

# Refuse breaking changes to the v1 endpoint API

## Goal

Turn the reviewed Supported endpoint surface into a mechanical compatibility boundary, so a later
change cannot silently break downstream source while documentation still calls the API stable.

## Acceptance

- [x] The final release candidate records a deterministic machine-readable public-API baseline for
      every publishable crate, with Experimental items excluded by the same declarations users read.
- [x] A checker compares the current Supported surface with that baseline and refuses removed or
      incompatibly changed items while allowing additions reserved by non-exhaustive types and
      defaulted trait methods.
- [x] Mutation tests prove the checker rejects a removed function, changed parameter, newly required
      trait method and loss of a non-exhaustive reservation; an additive compatible change passes.
- [x] The registry-only endpoint fixture is compiled separately against packaged archives, so a
      source snapshot cannot replace consumer evidence.
- [x] The compatibility check runs in ordinary CI and the release gate, documents the deliberate
      process for a future major version, and adds no network dependency to an ordinary local run.
- [x] Focused checker tests, package rehearsal and the complete repository gate are green.

## Progress

- 2026-08-14: implementation started behind `A-41`. The compatibility algorithm and package-archive
  boundary are now specified independently of the final item set; the baseline itself remains
  blocked until `A-41` freezes the Supported endpoint configuration surface.
- 2026-08-14: after the `A-41` freeze and classification-closure review, the exact pinned rustdoc
  candidate records 4,184 Supported paths across twelve publishable packages in a 12.2 MB baseline.
  Forty-nine focused adversarial vectors cover removals, signatures, directional construction and
  trait changes, positive auto traits, re-exports, nested/cross-package classification, safe
  roll-forward and the archive-only boundary. The registry-shaped endpoint fixture compiles locked
  and offline against eight archives prepared as one unpublished release-candidate package set.
  Focused checks are green; only the coordinator's complete gate remains.
- 2026-08-14: the coordinator's integrated 55-step repository gate passed, including the isolated
  packaged-consumer rehearsal and all 49 compatibility-checker mutation tests.

## Notes

- The baseline is cut from the final candidate, not from a moving branch head. Stable `1.0.0`
  promotion separately requires the roadmap's product-boundary adoption evidence.
