---
id: A-37
title: Publish and verify the first release candidate
pillar: Application
status: in-progress
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

- [ ] Public and internal documentation are swept together: workspace versions, current adoption
      commands, generated comparison facts, roadmap status, changelog, What's New and reviewed
      release notes agree on `1.0.0-rc.2` while historical records remain unchanged.
- [ ] The changelog and reviewed notes describe the complete post-beta.7 boundary, including
      migration guidance, fixed defects, measurement limits, experimental surfaces and intentional
      omissions.
- [ ] The complete release gate passes on the frozen candidate. At most one local invocation is
      made; if infrastructure prevents a result, exact-SHA `main` CI and the protected tagged gate
      must supply the passing evidence. The local gate and retained load measurement are not
      repeated.
- [ ] Exact-SHA `main` CI, including Pages, passes before one annotated `v1.0.0-rc.2` tag is pushed;
      the refused `v1.0.0-rc.1` tag remains immutable and is never published or moved.
- [ ] The protected workflow publishes or verifies every public crate, a registry-only consumer,
      the installed optional-feature CLI, all five native CLI archives, SPDX documents, checksums,
      Pages and the non-draft GitHub prerelease from reviewed notes.
- [ ] Registry checksums, workflow evidence, tag object, release commit, release URL and portable
      asset set are recorded here before the story closes.
- [ ] No existing tag, package or asset is moved or overwritten, and no broader announcement or
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

## Notes

- The release workflow may create the repository prerelease and its exact assets. External review,
  direct outreach and broader publicity require a later explicit plan and authorization.
