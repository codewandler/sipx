---
id: A-37
title: Publish and verify the first release candidate
pillar: Application
status: done
priority: 1
design: docs/specs/release-workflow.md
epic: release
areas: [release, docs, sipx-cli, rc2]
predicate:
announcement:
note: first release candidate · reviewed docs, exact registry packages and portable artifacts
---

# Publish and verify the first release candidate

## Goal

Cut the complete post-beta.7 `main` tree as the first published release candidate, verify every
public package and binary artifact against that exact commit, retain any refused cut immutably, and
leave stable `1.0.0` and broader publicity as separate decisions.

## Acceptance

- [x] Public and internal documentation are swept together: workspace versions, current adoption
      commands, generated comparison facts, roadmap status, changelog, What's New and reviewed
      release notes agree on `1.0.0-rc.2` while historical records remain unchanged.
- [x] The changelog and reviewed notes describe the complete post-beta.7 boundary, including
      migration guidance, fixed defects, measurement limits, experimental surfaces and intentional
      omissions.
- [x] The complete release gate passes on the frozen candidate. At most one local invocation is
      made; if infrastructure prevents a result, exact-SHA `main` CI and the protected tagged gate
      must supply the passing evidence. The local gate and retained load measurement are not
      repeated.
- [x] Exact-SHA `main` CI, including Pages, passes before one annotated `v1.0.0-rc.2` tag is pushed;
      the refused `v1.0.0-rc.1` tag remains immutable and is never published or moved.
- [x] The protected workflow publishes or verifies every public crate, a registry-only consumer,
      the installed optional-feature CLI, all five native CLI archives, SPDX documents, checksums,
      Pages and the non-draft GitHub prerelease from reviewed notes.
- [x] Registry checksums, workflow evidence, tag object, release commit, release URL and portable
      asset set are recorded here before the story closes.
- [x] No existing tag, package or asset is moved or overwritten, and no broader announcement or
      external-review outreach occurs as part of the cut.

## Progress

- 2026-08-05: user authorized a final public/internal documentation sweep and the first prerelease
  cut after beta.7, followed by planning rather than starting external review. The selected version
  is `1.0.0-rc.1`: the post-beta tree is a compatibility candidate, not stable `1.0.0`.
- 2026-08-05: the release audit found one clean `main` worktree, six local commits ahead of the
  successful remote `main`, no existing RC.1 tag or package, an authenticated release account and
  the protected tag workflow that owns registry, Pages, five-target artifact and GitHub-release
  publication.
- 2026-08-05: the one local gate invocation on candidate `8a8206f` found a stale audio-claim test
  fixture: the module inventory still expected the pre-L16/pre-PCM four-module crate. The rest of
  that step's checker already proved L16 and PCM correctly, and the complete all-feature Clippy,
  workspace test, example and MSRV steps passed. The run later exhausted local disk in the feature
  matrix and correctly ended as an infrastructure non-result rather than a tree verdict. The
  fixture now includes `l16` and `pcm`, asserts L16 is ungated, and requires L16 encode/decode
  reachability; all 59 focused tests and the live audio-claim check pass. Per the no-repeat
  acceptance above, fresh exact-SHA CI and the tagged protected gate supply the complete result.
- 2026-08-05: exact-SHA `main` CI run `31049398629` passed at `660b68c`, including Pages. Annotated
  tag object `72e798e` created immutable `v1.0.0-rc.1` at that commit. Protected run `31050078126`
  passed the complete gate, then its release rehearsal refused `sipx-media`'s stale beta.7
  requirement on `sipx-transport`; publication had not started, so no registry package, portable
  asset or GitHub Release exists for RC.1. The tag remains untouched. RC.2 fixes the requirement
  forward and a failing-first live-graph test now exposes this exact defect before a protected cut.
- 2026-08-08: **RC.2 publication evidence, recorded from the live registry and repository.**
  - Exact-SHA `main` CI run `31051859061` succeeded 2026-08-05T22:12:43Z, before any tag was pushed.
  - Annotated tag object `b89c4e08a3f4991c04641251b7bf4536e7ca1a51` created immutable
    `v1.0.0-rc.2` at release commit `5dab4efa846686a1bd3a2ee6c348aedfa8d5cdaf`.
  - Protected run `31052427439` ("Publish release", ref `v1.0.0-rc.2`) succeeded 2026-08-05T22:21:21Z
    and owns the complete gate, registry publication, Pages and asset upload.
  - Registry: `sipx-sip` `7b43d24f8689ce1b…`, `sipx-transport` `7992ffc8fda03a82…`, `sipx-call`
    `3e1713020340c8cd…`, `sipx-media` `b08c19e1b545cbe7…`, `sipx-cli` `95ff7306edd54e69…`, each at
    `1.0.0-rc.2` and none yanked.
  - Release <https://github.com/codewandler/sipx/releases/tag/v1.0.0-rc.2> published
    2026-08-05T22:56:01Z, non-draft, flagged prerelease.
  - Portable asset set complete at five targets — `aarch64-apple-darwin`,
    `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc` and
    `x86_64-unknown-linux-musl` — each carrying its archive and SPDX 2.3 document, alongside one
    `SHA256SUMS`.
  - The refused `v1.0.0-rc.1` tag (`72e798e0…`) and its failed run `31050078126` remain untouched and
    unpublished; no rc.2 asset or package was moved or overwritten.

## Notes

- The release workflow may create the repository prerelease and its exact assets. External review,
  direct outreach and broader publicity require a later explicit plan and authorization.
