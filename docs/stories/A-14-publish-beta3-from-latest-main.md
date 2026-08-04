---
id: A-14
title: Publish 1.0.0-beta.3 from latest main
pillar: Application
status: done
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

- [x] Workspace packages, internal requirements, lockfile, current public adoption docs, maturity
      labels and generated comparison facts consistently name `1.0.0-beta.3`; historical release
      records remain historical.
- [x] Reviewed beta.3 release notes describe the changes since beta.2, the pre-1.0 API policy and
      intentional product boundaries without claiming new runtime features that did not land.
- [x] The complete local gate and exact-SHA `main` CI pass on a clean release commit.
- [x] Annotated tag `v1.0.0-beta.3` points to that exact commit and is pushed without moving any
      existing tag.
- [x] The protected ordinary release workflow publishes all eleven exact crates, verifies a clean
      registry-only consumer, crates.io-installed Opus CLI and Pages from the release commit, then
      creates the GitHub prerelease from the reviewed notes.
- [x] Registry checksums, CI and release run IDs, tag object and final GitHub URL are recorded here
      before the story is closed.

## Progress

- `v1.0.0-beta.2` is already an immutable published prerelease. The new cut therefore advances the
  prerelease number rather than moving that tag or attempting to overwrite registry versions.
- Release commit: `9c52ba4d9363ceab06c12783016673377ef67a2a`.
- The 35-step local gate passed, followed by exact-SHA `main` CI run
  [`30926549625`](https://github.com/codewandler/sipx/actions/runs/30926549625).
- Annotated tag `v1.0.0-beta.3` has tag object
  `357a1402441d6b5aacdbd4ab0b0a59f40db6cfd4` and peels to the release commit.
- Protected release run
  [`30927185252`](https://github.com/codewandler/sipx/actions/runs/30927185252) passed its complete
  gate, locked-package rehearsal, dependency-frontier publication, registry-only consumer,
  installed Opus CLI and exact-SHA Pages checks.
- crates.io checksums:
  - `sipx-audio`: `0d941e5ffede56e77376f0547fa165ace5de058fa5c0c0ba5ae0384308f4d26b`
  - `sipx-rtp`: `b5046066894e5a9ccf57384a1e315cf8e60124ec3cd79508fafcc8e1285fd9e7`
  - `sipx-sdp`: `e15ae8e7a22ed71625c340a2d13093b5375dbfd80c565ed89ebc6abef06707b7`
  - `sipx-sip`: `5dfdf5529388eb7fc4f5c0618c8e86122a350b6c15ef33d4ef4daa6126605b85`
  - `sipx-transport`: `30018c0fc36f54e82355452b93bcc0c23a0476eefd75ef6a10bfb0bfbbffdbd6`
  - `sipx-media`: `b3264249533d89c897bbac286df873567b274374e809f73404a726c4d5a50f9c`
  - `sipx-ua`: `03732d6c813692083a3db88f2e57bbbb7ad70ba89013b6d2b6bec1a65961c8ae`
  - `sipx-call`: `28f6e8bd1981bbb66cc3e3887333fea4d588cca87d8c6f90cc87f1908d662eb5`
  - `sipx-app-protocol`: `711e6bc63e3676e8164a09ac35cd9be49c1b434a474731fdd98c962f230b1ba1`
  - `sipx-app`: `b953c21f386e01be15ed3453ca87d670f0fd4d53e65bea016ea65641dfd79bcb`
  - `sipx-cli`: `776350fcd7c1533418e083738e24723658a0fcc507d50c52c712d7687769880f`
- Final prerelease: <https://github.com/codewandler/sipx/releases/tag/v1.0.0-beta.3>.

## Notes

- The user explicitly authorized a new release from the most recent version and requested that the
  beta.1 replay pipeline be removed. Commit `f667aa4` removed that one-purpose machinery from main.
