---
id: A-14
title: Publish 1.0.0-beta.3 from latest main
pillar: Application
status: in-progress
priority: 1
design: docs/specs/release-workflow.md
epic: app-sdk
areas: [release, docs, sipx-cli]
predicate:
announcement: [4, 5]
note: new immutable prerelease from latest main; beta.2 remains untouched
---

# Publish 1.0.0-beta.3 from latest main

## Goal

Ship the current `main` tree as a new immutable `1.0.0-beta.3` prerelease through the ordinary
protected GitHub release workflow. Do not move or overwrite the already-published beta.2 tag or
packages, and do not perform broader publicity.

## Acceptance

- [ ] Workspace packages, internal requirements, lockfile, current public adoption docs, maturity
      labels and generated comparison facts consistently name `1.0.0-beta.3`; historical release
      records remain historical.
- [ ] Reviewed beta.3 release notes describe the changes since beta.2, the pre-1.0 API policy and
      intentional product boundaries without claiming new runtime features that did not land.
- [ ] The complete local gate and exact-SHA `main` CI pass on a clean release commit.
- [ ] Annotated tag `v1.0.0-beta.3` points to that exact commit and is pushed without moving any
      existing tag.
- [ ] The protected ordinary release workflow publishes all eleven exact crates, verifies a clean
      registry-only consumer, crates.io-installed Opus CLI and Pages from the release commit, then
      creates the GitHub prerelease from the reviewed notes.
- [ ] Registry checksums, CI and release run IDs, tag object and final GitHub URL are recorded here
      before the story is closed.

## Progress

- `v1.0.0-beta.2` is already an immutable published prerelease. The new cut therefore advances the
  prerelease number rather than moving that tag or attempting to overwrite registry versions.

## Notes

- The user explicitly authorized a new release from the most recent version and requested that the
  beta.1 replay pipeline be removed. Commit `f667aa4` removed that one-purpose machinery from main.
