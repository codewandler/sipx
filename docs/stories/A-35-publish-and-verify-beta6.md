---
id: A-35
title: Publish and verify 1.0.0-beta.6
pillar: Application
status: done
priority: 1
design: docs/specs/release-workflow.md
epic: release
areas: [release, docs, sipx-cli, beta6]
predicate:
announcement: 6
note: published immutable beta · exact registry, CLI, Pages and prerelease evidence recorded
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
- [x] Exact-SHA `main` CI, including Pages, passes on the clean release commit before one annotated
      `v1.0.0-beta.6` tag is pushed. Per explicit user direction, no second local full gate or load
      measurement is run; the protected tagged workflow runs its required gate before publication.
- [x] The protected ordinary workflow rehearses and publishes every exact public crate, verifies a
      registry-only consumer and installed Opus CLI, checks Pages from the release SHA, and creates
      the non-draft GitHub prerelease from the reviewed notes.
- [x] Registry checksums, CI and release run IDs, tag object, release commit and GitHub URL are
      recorded here before the story closes.
- [x] No existing tag or package is moved or overwritten, and no broader announcement occurs.

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
- 2026-08-05: release commit `c1a5b6e82035518db5fabfe20a1303f11e294078` passed exact-SHA
  [`main` CI run `31017930979`](https://github.com/codewandler/sipx/actions/runs/31017930979),
  including Pages deployment job `92347692087`, before the tag was created.
- Annotated tag object `814b3fff0265779277c51d952689b8026285f46d` peels to that release
  commit. Protected ordinary workflow run
  [`31018659658`](https://github.com/codewandler/sipx/actions/runs/31018659658) passed; job
  `92349343969` ran the tagged gate, locked rehearsal, bounded publication, exact registry consumer,
  installed CLI and Pages verification. Job `92357312727` created or verified the GitHub prerelease.
- crates.io recorded these exact `1.0.0-beta.6` checksums:

  | Crate | Registry checksum |
  |---|---|
  | `sipx-audio` | `feb05c9b91c33de3808ebf61d15a49548db6e645282bda28cc5c515fe35de3fb` |
  | `sipx-rtp` | `3046b533765cef1527904aa76214c9870df3935c2d51767f1bf83e7d6a99c4f0` |
  | `sipx-sdp` | `b7c8d4145824724603bd995d9fe088283acfae075da3b1a9a42f561e011e0bad` |
  | `sipx-sip` | `2d22a5c5663646a2ac5e5949b00bcffce28743fa7a7bef9f60a1a743b3ac3622` |
  | `sipx-transport` | `d946d8e15bf4d6d8c0da5262e079eebfb04c4f45e8ac35be854d2c44b9b558c2` |
  | `sipx-media` | `83b07647197ef89cd3c9a926c5ca43ec6a8162fe6f64d1629111d2803860c287` |
  | `sipx-ua` | `7f027626a9a21b4d86c2a78fd785f70a6132668f136ea15c62c64381ab11fe6e` |
  | `sipx-call` | `6b588347d74c3a1b8afc9a18aea126bb53e1c761aee164fa9359e68c82b49f8e` |
  | `sipx-app-protocol` | `1e162b3a42bd5f4a5f78ca723cbc72f960175b7927c280eeaf408f9596f9bfdf` |
  | `sipx-app` | `4f32a3a6c003dcd03180b1fc77f6efa67a5990194995e41762f0e1018515827f` |
  | `sipx-cli` | `72b83a14941b288ec1ed3ebd4109aba09ea15d3f481627220a9258afcabbf11d` |
  | `sipx-testkit` | `e697e959a8f6f3bceba1b05b2ef3224b608a902fc9ef0f6f8bd83987f5b817bf` |
- GitHub published the non-draft prerelease at
  `https://github.com/codewandler/sipx/releases/tag/v1.0.0-beta.6`. Its reviewed body matches
  `docs/releases/1.0.0-beta.6.md` after trimming the API's trailing newline. Public Pages returns
  the beta.6 landing page. No existing tag or package moved, and no broader announcement was made.

## Notes

- The retained endpoint dataset describes the artifact that was actually measured. Its peer
  direction stays `not_measured`; changing a release label must not manufacture new evidence.
