---
id: X-92
title: Pass the provenance denylist to the release gate
pillar: Build
status: done
priority: 1
design: docs/specs/release-workflow.md
epic: conformance
areas: [release, ci]
predicate:
announcement: [4, 5]
note: beta-1 release blocker · the tagged gate could not read the configured organization secret
---

# Pass the provenance denylist to the release gate

## Goal

Make the protected release job run the same mandatory provenance check as ordinary CI before any
registry write, using the externally configured denylist without broadening the credential scope.

## Acceptance

- [x] Failed release run `30906820031` is recorded as the failing-first observation: the exact tag
      and Cargo credential checks passed, then the gate exited `2` because `SIPX_DENYLIST` was not
      present; no package or GitHub Release was published.
- [x] The denylist secret is exposed only to the complete-gate step, and the normative release
      contract names that required input without putting the denylist in the repository.
- [x] The release-workflow structural suite fails if that step loses or renames the secret mapping.
- [x] An empty denylist is refused before the keep-going gate starts, so the same configuration
      failure cannot spend another full cold-gate run collecting unrelated results.
- [x] The complete local gate and exact-sha GitHub CI pass before a new immutable prerelease tag is
      cut; `v1.0.0-beta.1` is not moved.

## Progress

- Release run `30906820031` reached the complete gate from annotated tag `v1.0.0-beta.1` at commit
  `3ab81709c7a235831638c62eba5fe73ce9eb7773`. The provenance step alone failed because the release
  workflow did not map the configured Actions secret into that step. GitHub Actions does not expose
  a secret as a process environment variable merely because the secret exists.
- Every irreversible step followed the gate and was skipped: package rehearsal, registry
  publication, exact registry consumer, Pages proof and GitHub prerelease. The required workflow
  fix therefore advances the workspace to the next prerelease rather than moving the failed tag.
- The failed run reported the missing input after 33 seconds but the keep-going gate returned only
  after another 12 minutes and 4 seconds. The release step now preflights that one required value;
  the gate itself retains its diagnostic keep-going behavior for ordinary verification failures.
- The complete 32-step local gate passed on the corrected `1.0.0-beta.2` candidate. Exact-SHA CI
  run `30910251565` then completed green at `9a0f42ec6eb42f4831ae1a98388f85b9f063d0ac`, including
  provenance, workflow guards, cargo-deny, feature matrix, package-path proofs and Pages deployment.

## Notes

- `SIPX_DENYLIST` is verification input, not registry publication authority. It belongs on the gate
  step; `CARGO_REGISTRY_TOKEN` remains confined to its presence check and publication step.
