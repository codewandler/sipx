---
id: A-43
title: Make partial-release recovery work for stable v1
pillar: Application
status: done
priority: 1
design: docs/specs/release-workflow.md
epic: release
areas: [release, ci]
predicate:
announcement:
note: recovery still names obsolete prerelease jobs and would recreate stable v1 as a prerelease
---

# Make partial-release recovery work for stable v1

## Goal

Keep the protected partial-publication recovery path usable for both prereleases and stable
`1.0.0`. Recovery must authenticate the current ordinary-release jobs and must derive the GitHub
Release kind from the immutable release version exactly as the ordinary workflow does.

## Acceptance

- [x] Failing-first workflow tests expose both defects in the current recovery controller: it
      authenticates obsolete `prerelease` job names, and it always verifies or creates a GitHub
      prerelease even when the immutable version is stable.
- [x] Failed-run authorization requires exactly one current `publish and verify release` job and
      one skipped `publish or verify GitHub release` job before recovery may see the registry
      credential.
- [x] Recovered GitHub Release handling derives its expected kind from `RELEASE_VERSION`: a version
      with a prerelease suffix uses `--prerelease`, while stable `1.0.0` uses `--latest`; an
      existing release must have that same computed kind.
- [x] The static workflow checker and adversarial tests refuse either obsolete job names or a
      recovery path hard-coded to one release kind.
- [x] `python3 scripts/test-release-workflow.py`, `python3 scripts/check-release-workflow.py --check`,
      `./scripts/check-provenance.sh`, formatting and diff checks pass in the implementation
      worktree.
- [x] The complete repository gate passes after integration.

## Progress

- 2026-08-14: selected during the stable-v1 release audit. The normative release workflow already
  requires stable releases to be non-prerelease records, but the recovery workflow and its checker
  still encode the earlier prerelease-only job names and release kind.
- 2026-08-14: the two focused tests failed first against those exact defects. The implementation
  now authenticates the current ordinary jobs and shares their stable-versus-prerelease decision
  shape. All 58 release-workflow tests, the live workflow check, YAML parsing, provenance, Python
  compilation and `git diff --check` pass; the coordinator-owned complete gate remains open.
- 2026-08-14: the integrated stable tree passed all 55 gate steps in 14m04s; this story closes.

## Notes

- This changes recovery authority and release-record classification only. It does not create a
  commit, tag, package, GitHub Release or announcement.
- The recovery workflow's pinned tag object and controller toolchain belong to its existing
  partial-publication incident contract and are not widened by this story.
