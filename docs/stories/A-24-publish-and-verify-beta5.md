---
id: A-24
title: Publish and verify 1.0.0-beta.5
pillar: Application
status: done
priority: 1
design: docs/specs/release-workflow.md
epic: release
areas: [release, docs, sipx-cli, beta5]
predicate:
announcement: 6
note: published immutable beta · exact registry, CLI, Pages and prerelease evidence recorded
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
- [x] The complete local gate and exact-SHA `main` CI, including Pages, pass on the clean release
      commit before one annotated `v1.0.0-beta.5` tag is pushed.
- [x] The protected ordinary workflow rehearses and publishes every exact public crate, verifies a
      registry-only consumer and installed Opus CLI, checks Pages from the release SHA, and creates
      the non-draft GitHub prerelease from the reviewed notes.
- [x] Registry checksums, CI and release run IDs, tag object, release commit and GitHub URL are
      recorded here before the story closes.
- [x] No existing tag or package is moved or overwritten, and no broader announcement occurs.

## Progress

- 2026-08-05: user authorized cutting and releasing the next beta. The existing tags establish
  `1.0.0-beta.5` as the next immutable version. The ordinary workflow remains the only publication
  path; it requires a clean main-contained annotated tag and reviewed release notes.
- 2026-08-05: the 86-commit post-beta.4 audit is recorded in `X-101`. The beta.5 candidate advances
  every workspace package and internal requirement together, publishes `sipx-testkit`, refreshes
  generated self-comparison facts, and keeps the live credentialed realtime proof and comparative
  load limits explicit in both adoption notes and the reviewed release record.
- 2026-08-05: the complete local gate passed 36 of 36 steps on release commit
  `0133a464f2722b8c8495ccf36b192c6c9642b3bf`, followed by exact-SHA `main` CI run
  [`31001569564`](https://github.com/codewandler/sipx/actions/runs/31001569564). Its Pages deployment
  job `92292231866`, nightly fuzz job `92291447306`, native-browser proof job `92291447169` and all
  other required jobs passed before the tag was created. Earlier run `31000287525` found the
  same-day nightly atomic-API deprecation before any tag or publication; commit `0133a46` added the
  narrow minimum-Rust compatibility bridge and received the complete replacement evidence above.
- Annotated tag object `cd28f17e9f46412d0c82ed8e670f7bee6adfe066` peels to the release commit.
  Protected ordinary workflow run
  [`31002180570`](https://github.com/codewandler/sipx/actions/runs/31002180570) passed; job
  `92293425885` ran the complete tagged gate, locked rehearsal, bounded publication, exact registry
  consumer, installed Opus CLI loopback and Pages verification. Job `92298661904` created or
  verified the GitHub prerelease.
- crates.io recorded these exact `1.0.0-beta.5` checksums:

  | Crate | Registry checksum |
  |---|---|
  | `sipx-audio` | `d36bd298a24405ecfb825d70de5cd1ee32611fcc35de222e77d0375d998260e4` |
  | `sipx-rtp` | `2cd5192acadb03318e37c8813536c1adf8b8cb43c04157c6ecee96203ff8985a` |
  | `sipx-sdp` | `b48e480ad2de69286a59a700c522d4309ff5ec96a65107e174f6e27f7c94bbab` |
  | `sipx-sip` | `ebbaf2e8ca3477128e8899a26e67874e4cd55efc4cbd6f2929034e5586efdb25` |
  | `sipx-transport` | `d48d1ebbee8c484d74323e3068f755381934c1fd61e92172613e5ac0a9f15d0c` |
  | `sipx-media` | `140a318f6245ab05afda8e7e0f6019c0fa1b47b2dc6aea977062a8a94d9029a0` |
  | `sipx-ua` | `7435fcdde8d9cda0953d6beeeb32147a2c9c9fd67d9bc0e4bcb42881e12a4ffb` |
  | `sipx-call` | `49e0302757cf2625c542ab230c2b9734892fffd62b86fb29c59db1d39f8019d6` |
  | `sipx-app-protocol` | `3a5bf2538b1477ca34b04aa303f4fba534d6975d1f3d10b65884b5a7d70f23b3` |
  | `sipx-app` | `fdddc98ec72162920d9616593c35731eedb869c76817573cbfd50eb0b99e55b0` |
  | `sipx-cli` | `194985cec195e7a895c90afeefcc3f4538dc07b86aae17b3fd51953846fb25f2` |
  | `sipx-testkit` | `cbca4ae97e18949a95a9d0a42e673739e41e283ac50ecd587d36986c74474a67` |
- GitHub published the non-draft prerelease at
  `https://github.com/codewandler/sipx/releases/tag/v1.0.0-beta.5`. Its reviewed body matches
  `docs/releases/1.0.0-beta.5.md` after trimming the API's trailing newline. Public Pages returned
  the beta.5 landing page, generated API reference and horizontally scrollable comparison page.
  No existing tag or package moved, and no broader announcement was made.

## Notes

- `A-23` is an opt-in credentialed live proof, not part of the default test matrix. This release may
  ship the implemented bridge while saying that live-endpoint interoperability evidence is pending.
