---
id: X-155
title: Make package rehearsal independent of Cargo cache state
pillar: Build
status: in-progress
priority: 0
design: docs/specs/release-rehearsal.md
epic: release
areas: [release, ci, tests]
predicate:
announcement:
promotion:
note: the locked package consumer must establish its own cache before proving an offline build
---

# Make package rehearsal independent of Cargo cache state

## Goal

Make the packaged endpoint-consumer proof succeed from both cold and warm Cargo caches without
allowing its resolved dependency identities to drift from the committed release lockfile.

## Acceptance

- [x] A failing-first regression records that pruning the seeded workspace lock under `--offline`
      can fail when a retained locked archive is not cached.
- [x] The rehearsal fetches the committed workspace lock with network access, refuses every
      retained consumer identity absent from that lock, then resolves, compiles and inspects only
      under `--locked --offline`.
- [x] The focused API-compatibility tests and packaged endpoint consumer pass from an isolated cold
      Cargo home.
- [x] The stable patch manifests, changelog, release note and generated public documentation agree
      on the release that supersedes the unpublished `v1.0.0` cut.
- [ ] The complete local acceptance gate passes.

## Progress

- 2026-08-15: protected release run `31845118483` passed 54 of 55 gate steps from immutable
  `v1.0.0`, then the packaged endpoint consumer could not download a retained locked dependency
  because its first metadata command required an already-warm cache. Package rehearsal,
  publication, assets and GitHub Release creation were skipped; no `1.0.0` package was uploaded.
- 2026-08-15: the new regression failed before implementation, then all 52 API-compatibility tests
  passed. The complete package-only proof passed against eight archives from an empty Cargo home,
  and all 25 `scripts/test-*.py` suites passed. Version, maturity, comparison, website, link,
  provenance, story-closure and formatting checks are green; the one final complete gate remains.

## Notes

- The immutable `v1.0.0` tag is not moved. The correction ships forward as the first stable patch.
- Network access may populate Cargo's cache but may not select a dependency identity outside the
  release candidate's committed lockfile. The final consumer proof remains locked and offline.
