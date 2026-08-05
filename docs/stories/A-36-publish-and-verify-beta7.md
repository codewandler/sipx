---
id: A-36
title: Publish and verify 1.0.0-beta.7
pillar: Application
status: in-progress
priority: 1
design: docs/specs/release-workflow.md
epic: release
areas: [release, docs, sipx-cli, beta7]
predicate:
announcement: 6
note: next immutable beta from integrated main · exact registry, CLI, Pages and prerelease evidence
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
- [ ] Exact-SHA `main` CI, including Pages, passes on the clean release commit before one annotated
      `v1.0.0-beta.7` tag is pushed. Per explicit user direction, no duplicate local full gate or
      load measurement is run; the protected tagged workflow runs its required gate before
      publication.
- [ ] The protected ordinary workflow rehearses and publishes every exact public crate, verifies a
      registry-only consumer and installed Opus CLI, checks Pages from the release SHA, and creates
      the non-draft GitHub prerelease from the reviewed notes.
- [ ] Registry checksums, CI and release run IDs, tag object, release commit and GitHub URL are
      recorded here before the story closes.
- [ ] No existing tag or package is moved or overwritten, and no broader announcement occurs.

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

## Notes

- The protected tagged workflow is the authoritative full release gate for this cut. The earlier
  local invocation ended as an infrastructure non-result after its disk precondition changed; it
  is not represented as either a pass or a repository failure and will not be repeated.
