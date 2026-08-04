---
id: X-94
title: Resume a partial release with fixed controller tooling
pillar: Build
status: in-progress
priority: 1
design: docs/specs/release-workflow.md
epic: conformance
areas: [release, ci]
predicate: 4
announcement: [4, 5]
note: beta.2 published four leaf crates before the tag-bound resume verifier rejected Cargo's valid clean VCS record
---

# Resume a partial release with fixed controller tooling

## Goal

Recover an immutable, partially published release when the package bytes are correct but the
tag-bound release controller has a defect, without moving the tag, changing any package byte,
exposing the registry credential outside the protected environment, or turning `main` into generic
publication authority.

## Acceptance

- [x] A failing-first archive test records Cargo's clean-checkout VCS shape: an omitted `git.dirty`
      means clean, `dirty: true` means dirty, and a present non-boolean value is refused.
- [x] A separate protected recovery workflow takes an exact annotated tag and the failed release run
      ID. It uses controller tooling from its exact `main` workflow commit against a separate clean
      checkout of the immutable tag; controller files never enter the release checkout or its
      package archives.
- [x] Before any registry write, the workflow proves that the named failed run belongs to the exact
      tag commit and original release workflow, and that its full gate and locked rehearsal passed
      before its publication step failed.
- [x] The helper's recovery authority is distinct from tag-push authority and is bound to the
      protected recovery workflow, exact tag, release SHA, failed run ID, controller SHA and positive
      Actions run identity. Branch CI, another repository/workflow, a moved tag or a dirty release
      checkout dispatches no upload.
- [ ] Every already-visible crate is reproduced from the tag and matches crates.io byte-for-byte
      before the missing dependency frontier is published. Publication remains bounded, then the
      exact consumer/Opus CLI, Pages and idempotent GitHub-prerelease proofs run unchanged.
- [ ] Structural mutation tests hold both authority paths, the recovery evidence query, credential
      scopes and ordering; `./scripts/gate.py` is green.

## Progress

- Protected release run `30912030744` passed the complete 32-step gate and locked package rehearsal,
  then published `sipx-audio`, `sipx-rtp`, `sipx-sdp` and `sipx-sip` at `1.0.0-beta.2`. Its second
  frontier invocation stopped before another upload because the verifier required an explicit
  `dirty: false` value that Cargo omits for a clean checkout.
- Fresh archives reproduced from immutable tag `v1.0.0-beta.2` at
  `4aa64caaf38c59961b746a2cfabbed1bc6394501` match the four crates.io downloads by SHA-256. The other
  seven exact versions are absent. Recovery must preserve those facts rather than reclassifying the
  failed run as unpublished.
- The controller now accepts an explicit release root and a distinct recovery authorization. Its
  54 focused tests cover omitted/dirty/malformed VCS facts, separated roots, every controller and
  release identity, checksum refusal, and the rule that recovery sees at least one already-visible
  exact package before it can dispatch anything. The workflow has 26 structural and mutation tests;
  it pins tag object `04a19dff6a7d7b6c072c98d18ad4b42407955d4b` and Cargo `1.97.1`, the identities
  used by the failed run.

## Notes

- This is disaster recovery for unchanged release bytes, not authority to repair a crate in place.
  A package-content change still requires a new prerelease version under `A-12`.
- The ordinary tag-push workflow remains the only first-publication entry. Recovery must name a
  failed run that already crossed the full gate and rehearsal boundary for the same immutable tag.
