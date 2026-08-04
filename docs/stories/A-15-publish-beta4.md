---
id: A-15
title: Publish and verify 1.0.0-beta.4
pillar: Application
status: backlog
priority: 10
design: docs/specs/release-workflow.md
epic: release
areas: [release, docs, sipx-cli, beta4]
predicate:
announcement: 6
note: beta.4 capstone · immutable tag, exact registry consumer, installed Opus CLI, Pages and GitHub prerelease
---

# Publish and verify 1.0.0-beta.4

## Goal

Cut the completed beta.4 wave through the ordinary protected GitHub release workflow and record
enough evidence to prove every public artifact came from one immutable release commit.

## Acceptance

- [ ] Every other story tagged `beta4` is `done`; `M-38` is closed; alpha integrity and all beta.4
      announcement predicates are met before the tag is created.
- [ ] Workspace versions, lockfile, changelog, reviewed release notes, README, public site,
      comparison facts and explicit omissions consistently name `1.0.0-beta.4` while historical
      release records remain unchanged.
- [ ] The complete local gate and exact-SHA `main` CI, including Pages, pass on the clean release
      commit before one annotated `v1.0.0-beta.4` tag is pushed.
- [ ] The protected ordinary workflow rehearses and publishes all exact public crates, verifies a
      registry-only consumer and crates.io-installed Opus CLI, checks Pages from the release SHA,
      and creates a non-draft GitHub prerelease from the reviewed notes.
- [ ] Registry checksums, CI and release run IDs, tag object, release commit and GitHub URL are
      recorded before this story closes.
- [ ] The release text claims the bounded browser-audio and non-ICE connectivity paths proven by
      the wave and explicitly excludes TURN-required networks, video, browser APIs, data channels
      and stable-1.0 API compatibility.
- [ ] No broader announcement occurs without separate explicit authorization.

## Progress

- Blocked on the other nine beta.4 stories and the `M-38` epic exit.

## Notes

- This is a new immutable prerelease. It never moves or overwrites beta.2 or beta.3 artifacts.
