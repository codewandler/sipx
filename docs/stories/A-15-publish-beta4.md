---
id: A-15
title: Publish and verify 1.0.0-beta.4
pillar: Application
status: done
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

- [x] Every other story tagged `beta4` is `done`; `M-38` is closed; alpha integrity and all beta.4
      announcement predicates are met before the tag is created.
- [x] Workspace versions, lockfile, changelog, reviewed release notes, README, public site,
      comparison facts and explicit omissions consistently name `1.0.0-beta.4` while historical
      release records remain unchanged.
- [x] The complete local gate and exact-SHA `main` CI, including Pages, pass on the clean release
      commit before one annotated `v1.0.0-beta.4` tag is pushed.
- [x] The protected ordinary workflow rehearses and publishes all exact public crates, verifies a
      registry-only consumer and crates.io-installed Opus CLI, checks Pages from the release SHA,
      and creates a non-draft GitHub prerelease from the reviewed notes.
- [x] Registry checksums, CI and release run IDs, tag object, release commit and GitHub URL are
      recorded before this story closes.
- [x] The release text claims the bounded browser-audio and non-ICE connectivity paths proven by
      the wave and explicitly excludes TURN-required networks, video, browser APIs, data channels
      and stable-1.0 API compatibility.
- [x] No broader announcement occurs without separate explicit authorization.

## Progress

- The other nine beta.4 stories and the `M-38` epic exit were complete before publication. Release
  metadata and public documentation consistently describe the beta.4 boundary; the protected
  workflow then produced and independently verified the immutable artifacts recorded below.
- The complete local gate passed 36 of 36 steps. Exact-main workflow run `30955349652` passed at
  release commit `160a752cab18880ecf56efabbdbe84374249582e`, including Pages deployment job
  `92147482555` and native-browser proof job `92146965728`, before the tag was created.
- Annotated tag object `0d3855ac7aee553b7e57b8a7c8fe3084bc523521` peels to that release commit.
  Protected ordinary workflow run `30955847937` passed; job `92148554820` ran the complete tag gate,
  locked rehearsal, bounded publication, exact registry consumer, crates.io-installed Opus CLI
  loopback and Pages verification. Job `92152441127` created or verified the GitHub prerelease.
- crates.io recorded these exact `1.0.0-beta.4` checksums:

  | Crate | Registry checksum |
  |---|---|
  | `sipx-audio` | `1ecc181e39f95e6588cf35b1fc26f53537377c590bdcb472eb6c7e7362f2255c` |
  | `sipx-rtp` | `0b2db56b01b8a7a46e923a271319449ea13d1ae45db18d64766940f07906a94c` |
  | `sipx-sdp` | `4194086f5f26151c74487d92ed351c4bdffa012b4c3ddd836675e3c61e4d12f7` |
  | `sipx-sip` | `1bd71906b14fdd3fa56dc72361379862b39aafc897e5e6f8078b4888af49c280` |
  | `sipx-transport` | `fd2a61fd43b9e49e633d867f361b8cc56d3be2326671cc2e0c6d8ffa1626563f` |
  | `sipx-media` | `fc7e681b18b073959e0551510674d1573fed8c10716b748e3b220064e52bb33d` |
  | `sipx-ua` | `97311879dfce72d80c44b8db5b0c821d6f3b132d73d20992edd395ae7b4616b4` |
  | `sipx-call` | `5cece9f7ad6aae575a5a519e60ae5a4253a67557535012bf9693644dc9bb60c1` |
  | `sipx-app-protocol` | `0e1abf8f9d15a12959f37c97df39f0cb4c51865b46a4afa75cc61414f358e3b0` |
  | `sipx-app` | `3a116b278dea498c28a20e414e79090a45db044682ebcafb923c80820da20a00` |
  | `sipx-cli` | `c77d494bd693a064ebf7841017ba140f7994414b4249f72e862a7805685335d4` |
- GitHub published the non-draft prerelease at
  `https://github.com/codewandler/sipx/releases/tag/v1.0.0-beta.4`. Its reviewed body matches
  `docs/releases/1.0.0-beta.4.md` after trimming the API's trailing newline. Public Pages returned
  the beta.4 getting-started guide, `sipx-call` API reference and executable browser-audio proof.

## Notes

- This is a new immutable prerelease. It never moves or overwrites beta.2 or beta.3 artifacts.
