---
id: A-36
title: Publish and verify 1.0.0-beta.7
pillar: Application
status: done
priority: 1
design: docs/specs/release-workflow.md
epic: release
areas: [release, docs, sipx-cli, beta7]
predicate:
announcement: 6
note: published immutable beta · exact registry, CLI, Pages and prerelease evidence recorded
---

# Publish and verify 1.0.0-beta.7

## Goal

Cut the complete post-beta.6 `main` tree as a new immutable `1.0.0-beta.7` prerelease through the
ordinary protected release workflow, verify every public artifact against the release commit, and
leave beta.6 and its historical record unchanged.

## Acceptance

- [x] All worktrees are resolved into one `main`; the release boundary includes the complete
      post-beta.6 transaction, listener, identity and URI integration wave plus its final parser
      and generated-compliance hardening.
- [x] Workspace packages, internal requirements, lockfile, current README/site guidance, generated
      comparison facts, roadmap status, changelog and reviewed release notes consistently name
      `1.0.0-beta.7` while historical release records remain unchanged.
- [x] Reviewed release notes describe exact outgoing INVITE cancellation, exact cleartext listener
      selection, typed privacy/identity fields and lossless URI editing, including migration
      guidance and intentional limits.
- [x] Exact-SHA `main` CI, including Pages, passes on the clean release commit before one annotated
      `v1.0.0-beta.7` tag is pushed. Per explicit user direction, no duplicate local full gate or
      load measurement is run; the protected tagged workflow runs its required gate before
      publication.
- [x] The protected ordinary workflow rehearses and publishes every exact public crate, verifies a
      registry-only consumer and installed Opus CLI, checks Pages from the release SHA, and creates
      the non-draft GitHub prerelease from the reviewed notes.
- [x] Registry checksums, CI and release run IDs, tag object, release commit and GitHub URL are
      recorded here before the story closes.
- [x] No existing tag or package is moved or overwritten, and no broader announcement occurs.

## Progress

- 2026-08-05: user directed that every worktree be integrated into clean `main`, authorized the next
  beta release, and explicitly refused another duplicate local release-gate or measurement run.
  Only the main worktree remains; five integrated commits follow beta.6.
- 2026-08-05: the release boundary retains the existing bounded responder measurement unchanged.
  It adds no peer-direction, secure-transport, media or general performance claim.
- 2026-08-05: workspace and internal package versions, lockfile, current adoption guidance,
  comparison facts, compliance facts, maturity output, roadmap, changelog and reviewed release
  notes agree on beta.7. Focused metadata, formatting, generated-content, release-workflow and
  provenance checks pass. No local full gate, package rehearsal or load run was started.
- 2026-08-05: release commit `0034ee364252aef2c996eb24278c6cd70cb3c48f` passed exact-SHA
  [`main` CI run `31028302473`](https://github.com/codewandler/sipx/actions/runs/31028302473),
  including Pages deployment job `92382954905`, before the tag was created.
- Annotated tag object `b9e30599ec093ce5585c73d56229ba6c93b1c9d9` peels to that release
  commit. Protected ordinary workflow run
  [`31029004601`](https://github.com/codewandler/sipx/actions/runs/31029004601) passed; job
  `92384647645` ran the tagged gate, locked rehearsal, bounded publication, exact registry consumer,
  installed CLI and Pages verification. Job `92391035282` created or verified the GitHub prerelease.
- crates.io recorded these exact `1.0.0-beta.7` checksums:

  | Crate | Registry checksum |
  |---|---|
  | `sipx-audio` | `ddaadfa6a30d45a0867501e077d8278644b4697ce9b1aef70d29eab7be74755f` |
  | `sipx-rtp` | `34db2d535daf33f0e327b51d12059a0fb1a8ed277961ba1f2b1f6c29c753c2af` |
  | `sipx-sdp` | `07193890460d21f7e6461a34e138afae8335cd321d8794f1c2e774cd77408856` |
  | `sipx-sip` | `7ce12efac001144e963c66af25912c242df17e315431734a6b8d87e1ffff9a9f` |
  | `sipx-transport` | `d337c422851dfdb53f118071ef0677cde1ba10903ee4c73bdacd0ce8e53e0908` |
  | `sipx-media` | `f9c43341ae1d4b30edfef03ec95f060b19bbd34fc93ce75e34f3b765d17319d7` |
  | `sipx-ua` | `54e8129c9ce5e11b6cc7074c3692a2aa188cbc84bfc4b9a8a3190cabd5992b96` |
  | `sipx-call` | `b81794d0a63c15dfbcc4b486007a4c506d9ff5751101a65a5b41799038522770` |
  | `sipx-app-protocol` | `d07ff45557c5872ab2e25957692e9115ed0a2561633775f7576a25421692a9ac` |
  | `sipx-app` | `55e9b9ee540c88635888a54a6711c81793e06fc0390a179ee67f4ce13020ee7e` |
  | `sipx-cli` | `fb04aad7b6f95bbeaf5eb3d9f6fee5084bdb0f300a8b76a5b1990f4873fbfc25` |
  | `sipx-testkit` | `3fd901ad5159c82c98c03b088fe227668330f26238a1c497f453956f833efce0` |
- GitHub published the non-draft prerelease at
  `https://github.com/codewandler/sipx/releases/tag/v1.0.0-beta.7`. Its body matches the reviewed
  release notes after trimming trailing newlines. Public Pages returns beta.7 from Getting Started
  and What's New. No existing tag or package moved, and no broader announcement was made.

## Notes

- The protected tagged workflow is the authoritative full release gate for this cut. The earlier
  local invocation ended as an infrastructure non-result after its disk precondition changed; it
  is not represented as either a pass or a repository failure and will not be repeated.
