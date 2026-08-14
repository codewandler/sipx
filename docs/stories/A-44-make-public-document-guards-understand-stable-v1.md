---
id: A-44
title: Make public-document guards understand stable v1
pillar: Application
status: done
priority: 1
design: docs/specs/release-workflow.md
epic: release
areas: [release, docs]
predicate:
announcement:
note: the public-doc guard requires prerelease wording even when the workspace version is stable
---

# Make public-document guards understand stable v1

## Goal

Let the public documentation cross the stable boundary without disabling the guard that keeps each
entry point honest about its release kind and compatibility promise.

## Acceptance

- [x] Failing-first tests show that a stable `1.0.0` documentation set is rejected by the existing
      prerelease-only requirements and that prerelease language cannot satisfy the stable branch.
- [x] Requirement selection is derived from the canonical workspace version: prerelease versions
      retain the existing public-prerelease and non-frozen policy, while stable versions require
      current-stable wording and the Supported v1 source-compatibility promise.
- [x] Shared adoption, capability and Experimental-surface requirements remain enforced in both
      branches; selecting stable does not weaken unrelated public-document checks.
- [x] Focused synchronization tests, the live `scripts/sync-website.py --check`, provenance, Python
      compilation and diff checks pass in the implementation worktree.
- [x] The complete repository gate passes after integration.

## Progress

- 2026-08-14: selected during stable release preparation after the public synchronization guard
  was found to require the phrases `current public prerelease` and `Public APIs are not frozen`
  unconditionally.
- 2026-08-14: two stable-policy tests failed first: the prerelease-only guard rejected correct
  stable wording and did not report stale prerelease wording on stable. The version-derived guard
  now passes 37 synchronization tests and the live 28-region public-document check; shared
  adoption, capability and Experimental-policy mutations remain red in either release kind.
- 2026-08-14: the integrated stable tree passed all 55 gate steps in 14m04s; this story closes.

## Notes

- This story changes the guard, not the release prose. The stable documentation sweep remains the
  release coordinator's work after integration.
- No downstream product identity or evidence belongs in this structural checker.
