---
id: A-10
title: Publish the stable crate set and diagnostic CLI artifacts
pillar: Application
status: in-progress
priority: 1
design: docs/specs/release-artifacts.md
epic: app-sdk
areas: [release, docs, sipx-cli]
note: stable 1.0 owner · predicates, exact-SHA gate, registry set, five archives, SBOM, Pages and release evidence
---

# Publish the stable crate set and diagnostic CLI artifacts

## Goal

Publish the frozen stable v1 endpoint-library boundary as reproducible registry packages,
documentation and diagnostic binaries after every content and evidence predicate is satisfied.

## Acceptance

- [ ] Every stable evidence predicate generated in `docs/maturity.md` is met, every blocking row of
      the bounded v1 content contract is closed, and the finalized stable tree passes the complete
      local gate exactly once after all generated artifacts are synchronized.
- [ ] The reviewed stable note names the Supported endpoint boundary, Experimental surfaces,
      intentional omissions, verification limits and privacy-safe downstream-adoption result; all
      manifests, internal dependencies, lockfile, changelog, comparison, maturity, roadmap, board
      and website artifacts agree on the published stable patch; the reviewed rc.24 API baseline
      remains immutable and accepts the stable candidate under its same-major compatibility rule.
- [ ] Exact-SHA `main` CI, including Pages, passes before one reviewed annotated stable tag is
      pushed; no existing tag, package, release or asset is moved or overwritten.
- [ ] The protected workflow publishes or verifies every public crate in dependency order, an
      exact registry-only consumer, the installed optional-feature CLI, all five native archives,
      five SPDX documents, `SHA256SUMS`, exact-SHA Pages and a non-draft, non-prerelease release
      built from the reviewed note.
- [ ] Registry versions and checksums, workflow and gate evidence, annotated tag object, release
      commit and URL, Pages result, registry consumer and complete portable-asset set are recorded
      here before the story closes.

## Progress

- 2026-08-05: selected for the stable-release wave together with `P-14`. Four of the five evidence
  predicates were already present: post-alpha integrity, caller-bound reachability, a contract that
  shaped a refused breaking change, and two-profile interoperability for every released signalling
  transport. Separate-product adoption remained open.
- 2026-08-05: `docs/specs/release-artifacts.md` now fixes the five native target artifacts,
  no-optional-feature policy, static-musl proof, bounded call smoke test, SPDX identity, checksum
  aggregation and idempotent publication contract before workflow implementation.
- 2026-08-05: the tag workflow now builds and natively exercises the five-target matrix on exact
  release inputs, aggregates the ten published target files plus `SHA256SUMS`, creates stable or
  prerelease records from the version, and refuses to overwrite different existing bytes. The
  portable contract has 18 passing adversarial tests; its supervisor also completed a real call
  through the no-default-feature `sipx` binary. Stable publication remains an explicit release run,
  not an artifact-design task.
- 2026-08-05: `1.0.0-rc.2` is the first selected published exercise of that portable publication
  path; the RC.1 rehearsal stopped before publication.
  A successful candidate run can prove the mechanism and artifact bytes, but it cannot close this
  stable story.
- 2026-08-14: an authorized audit verified production adoption in a separately governed private
  downstream product without publishing its identity or proprietary details. `X-152` makes that
  product-boundary rule and the five stable evidence predicates generated. The bounded endpoint
  content and mechanical Supported-API baseline are complete; stable version and release-document
  synchronization are in progress. Tagging, pushing and publication remain deliberately unrun.
- 2026-08-14: the stable manifest sweep also found two explicit internal dependency versions in the
  unpublished WebAssembly member that predated the release checklist's count. They now follow the
  workspace at `1.0.0`; the final manifest scan covers every path dependency rather than trusting a
  transcribed total.
- 2026-08-15: immutable `v1.0.0` passed exact-SHA main CI but the protected cold-cache gate stopped
  before package rehearsal or any external write because the package-only consumer assumed one
  target-specific locked archive had already been cached. No `1.0.0` package or GitHub Release was
  published. `X-155` fixes forward; the normal protected path will publish `1.0.1` without moving
  or deleting the refused tag.
