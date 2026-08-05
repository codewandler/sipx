---
id: A-24
title: Publish and verify 1.0.0-beta.5
pillar: Application
status: in-progress
priority: 1
design: docs/specs/release-workflow.md
epic: release
areas: [release, docs, sipx-cli, beta5]
predicate:
announcement: 6
note: next immutable beta from current main · exact registry, CLI, Pages and prerelease evidence
---

# Publish and verify 1.0.0-beta.5

## Goal

Cut the complete post-beta.4 `main` wave as a new immutable `1.0.0-beta.5` prerelease through the
ordinary protected release workflow, verify every public artifact against the release commit, and
leave beta.4 and its historical record unchanged.

## Acceptance

- [x] The release boundary includes every completed post-beta.4 story on `main`; the credentialed
      live realtime-endpoint proof remains explicitly pending and is not implied by deterministic
      peer coverage.
- [x] Workspace packages, internal requirements, lockfile, current README/site guidance, generated
      comparison facts, roadmap status, changelog and reviewed release notes consistently name
      `1.0.0-beta.5` while historical release records remain unchanged.
- [x] Reviewed release notes describe the endpoint-services, operational, testing, comparative-load
      and realtime-agent changes since beta.4, including migration guidance and intentional limits.
- [ ] The complete local gate and exact-SHA `main` CI, including Pages, pass on the clean release
      commit before one annotated `v1.0.0-beta.5` tag is pushed.
- [ ] The protected ordinary workflow rehearses and publishes every exact public crate, verifies a
      registry-only consumer and installed Opus CLI, checks Pages from the release SHA, and creates
      the non-draft GitHub prerelease from the reviewed notes.
- [ ] Registry checksums, CI and release run IDs, tag object, release commit and GitHub URL are
      recorded here before the story closes.
- [ ] No existing tag or package is moved or overwritten, and no broader announcement occurs.

## Progress

- 2026-08-05: user authorized cutting and releasing the next beta. The existing tags establish
  `1.0.0-beta.5` as the next immutable version. The ordinary workflow remains the only publication
  path; it requires a clean main-contained annotated tag and reviewed release notes.
- 2026-08-05: the 86-commit post-beta.4 audit is recorded in `X-101`. The beta.5 candidate advances
  every workspace package and internal requirement together, publishes `sipx-testkit`, refreshes
  generated self-comparison facts, and keeps the live credentialed realtime proof and comparative
  load limits explicit in both adoption notes and the reviewed release record.

## Notes

- `A-23` is an opt-in credentialed live proof, not part of the default test matrix. This release may
  ship the implemented bridge while saying that live-endpoint interoperability evidence is pending.
