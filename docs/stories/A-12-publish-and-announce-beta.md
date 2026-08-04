---
id: A-12
title: Publish and verify 1.0.0-beta.2
pillar: Application
status: done
priority: 12
design: docs/roadmap.md
epic: app-sdk
areas: [release, docs, sipx-cli]
predicate:
announcement: [4, 5]
note: irreversible cut only after P-13, A-11 and explicit user authorization
---

# Publish and verify 1.0.0-beta.2

## Goal

Publish the one prerelease cut, verify its exact registry packages, public documentation and
installed CLI from the immutable release commit, and create its idempotent GitHub prerelease. Any
broader announcement remains a separate hypothetical decision.

## Acceptance

- [x] Before any registry write, hypothetical announcement-readiness predicates 1–3 are met,
      `A-11` is done, the workspace and every internal requirement name `1.0.0-beta.2`, and the full
      gate passes on the clean release commit. Completing this story then makes the generated report
      read five of five.
- [x] One immutable annotated tag supplies every published package; internal packages are published
      in derived dependency order and registry availability is polled under a finite bound.
- [x] A clean project builds exact crates.io versions with no path/Git override, and a
      crates.io-installed `sipx-cli` completes the bounded loopback proof.
- [x] README, public site, API docs and the reviewed release record name the unstable API policy and
      intentional omissions; the adoption surface leads with Rust crates and uses the CLI as proof.
- [x] The published pages are verified from the immutable release commit after the registry and CLI
      proofs; only then is the reviewed GitHub prerelease created or verified. The workflow posts no
      broader publicity.
- [x] A partial publication is resumed only from unchanged bytes; a required code change yanks the
      affected beta and increments the prerelease rather than moving the tag.

## Progress

- The reversible adoption surface is prepared for the beta release commit. README, the public site,
  crate-level API metadata and reviewed release notes state the pre-1.0 Supported/Experimental
  policy, lead with modular Rust crates and the executable CLI proof, and name the intentional
  product omissions. Exact crates.io requirements replace the historical alpha Git install on every
  current adoption page; historical alpha release notes remain intact.
- Before the first tagged cut, the workspace and every internal requirement named
  `1.0.0-beta.1`. On 2026-08-04, both the
  crates.io exact-version API and isolated Cargo queries classified all eleven planned
  `1.0.0-beta.1` package versions as absent, with no inconclusive result. The release helper
  correctly refused candidate check and dry-run modes before that release tree was committed; its
  separate dirty-content inspection and an explicit diagnostic workspace dry-run inspected bytes
  without declaring a release candidate.
- Documentation deployment is repository-defined: a push of the release commit to `main` builds
  the Docusaurus site and all-feature rustdoc, then publishes that one artifact to GitHub Pages. A
  tag push alone does not deploy it. Release verification must bind the successful Pages workflow's
  `headSha` to the annotated tag's commit and probe both the public guide and API surfaces before an
  authorized cut is considered complete. A reviewed release record lives at
  `docs/releases/1.0.0-beta.2.md`; it is the exact GitHub prerelease body, not authorization for
  broader publicity.
- The `codewandler` organization Actions secret `CARGO_REGISTRY_TOKEN` now grants this repository
  access without revealing or copying the credential. The exact-tag workflow keeps that token out
  of the gate and consumer jobs and exposes it only to the bounded publication step. The workflow
  has read-only repository authority; a dependent least-privilege job creates or verifies the one
  GitHub prerelease after every proof passes and posts nowhere else. Committing the release
  tree and creating/pushing its annotated tag remain separate authority boundaries; no local check
  or status edit substitutes for explicit user authorization to cross them.
- The reversible helper now enforces the partial-publication half of the final acceptance item. It
  reproduces clean tagged archives and compares their SHA-256 values with fresh canonical crates.io
  lockfile checksums before any later frontier, all-visible success or installed-consumer proof; a
  moved tag, changed archive or ambiguous registry probe dispatches no upload or install. The item
  remains open until the authorized beta publication actually exercises that recovery boundary, and
  any required fix still needs a new prerelease version.
- Annotated tag `v1.0.0-beta.1` reached protected release run `30906820031`, whose complete gate
  stopped before every irreversible step because the workflow did not pass `SIPX_DENYLIST` to the
  provenance check. Exact crates.io API queries found all eleven versions absent and no GitHub
  Release exists. The tag remains unchanged; the corrected candidate is `1.0.0-beta.2`.
- Annotated tag `v1.0.0-beta.2` resolves to release commit
  `4aa64caaf38c59961b746a2cfabbed1bc6394501`. Protected run `30912030744` passed the complete gate
  and rehearsal before publishing its first frontier; checksum-bound recovery run `30915437072`
  completed all eleven exact packages on attempt 5, then passed the clean registry consumer,
  crates.io-installed Opus CLI loopback and release-commit Pages proof. Its dependent job created
  the prerelease at <https://github.com/codewandler/sipx/releases/tag/v1.0.0-beta.2> on
  2026-08-04. No broader announcement was posted.

## Notes

- `beta.2` is the publishable prerelease cut and GitHub prerelease. The immutable `beta.1` tag is an
  unpublished failed cut. Broader publicity is a separate,
  explicitly authorized decision outside this story and workflow.
- The beta registry examples live only in the uncommitted release tree until the authorized cut;
  they must reach public `main` as part of the same bounded release window as publication.
