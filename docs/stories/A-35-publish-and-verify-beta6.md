---
id: A-35
title: Publish and verify 1.0.0-beta.6
pillar: Application
status: in-progress
priority: 1
design: docs/specs/release-workflow.md
epic: release
areas: [release, docs, sipx-cli, beta6]
predicate:
announcement: 6
note: next immutable beta from integrated main · exact registry, CLI, Pages and prerelease evidence
---

# Publish and verify 1.0.0-beta.6

## Goal

Cut the complete post-beta.5 `main` tree as a new immutable `1.0.0-beta.6` prerelease through the
ordinary protected release workflow, verify every public artifact against the release commit, and
leave beta.5 and its historical record unchanged.

## Acceptance

- [x] All worktrees are resolved into one clean `main`; the release boundary includes the complete
      post-beta.5 implementation and specification wave, while backlog designs remain explicitly
      described as plans rather than runtime capabilities.
- [x] Workspace packages, internal requirements, lockfile, current README/site guidance, generated
      comparison facts, roadmap status, changelog and reviewed release notes consistently name
      `1.0.0-beta.6` while historical release records remain unchanged.
- [x] Reviewed release notes describe responder hardening, validation accounting, the browser SDK
      contract and the planning-only media/application records, including migration guidance and
      intentional limits.
- [ ] Exact-SHA `main` CI, including Pages, passes on the clean release commit before one annotated
      `v1.0.0-beta.6` tag is pushed. Per explicit user direction, no second local full gate or load
      measurement is run; the protected tagged workflow runs its required gate before publication.
- [ ] The protected ordinary workflow rehearses and publishes every exact public crate, verifies a
      registry-only consumer and installed Opus CLI, checks Pages from the release SHA, and creates
      the non-draft GitHub prerelease from the reviewed notes.
- [ ] Registry checksums, CI and release run IDs, tag object, release commit and GitHub URL are
      recorded here before the story closes.
- [ ] No existing tag or package is moved or overwritten, and no broader announcement occurs.

## Progress

- 2026-08-05: user directed that every worktree be integrated into clean `main`, authorized the next
  beta release, and explicitly refused another duplicate local release-gate or measurement run.
  Only the single main worktree remains; seven integrated commits follow beta.5.
- 2026-08-05: beta.6 preparation advances all workspace packages together, retains the current-only
  bounded load evidence without rerunning it, refreshes generated self facts, and keeps browser,
  media and application plans distinct from shipped runtime behavior.
- 2026-08-05: version-derived public documents, comparison facts and maturity output are in sync;
  the focused public-content, comparison, maturity and provenance checks pass. No local full gate or
  new load run was started.

## Notes

- The retained endpoint dataset describes the artifact that was actually measured. Its peer
  direction stays `not_measured`; changing a release label must not manufacture new evidence.
