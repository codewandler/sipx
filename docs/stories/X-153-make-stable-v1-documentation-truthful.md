---
id: X-153
title: Make stable-v1 documentation truthful
pillar: Build
status: done
priority: 0
design: docs/specs/stable-v1-documentation.md
epic: release
areas: [docs, api, cli, release]
predicate:
announcement:
promotion:
note: current public prose and rustdoc must cross from prerelease policy to the Supported v1 compatibility promise
---

# Make stable-v1 documentation truthful

## Goal

Make every current public entry point and crate stability header describe the stable v1 contract
that the release actually makes: Supported Rust and CLI surfaces evolve compatibly throughout v1,
while explicitly Experimental roots remain outside that promise.

## Acceptance

- [x] A normative documentation contract distinguishes the frozen Supported v1 surface from
      Experimental roots and identifies generated and historical text that must not be hand-edited.
- [x] A failing-first consistency test rejects legacy pre-1.0 stability wording in current public
      pages or Rust documentation without treating historical release notes as current claims.
- [x] Every publishable library crate and the CLI describe their Supported contract as frozen for
      compatible v1 evolution; Experimental roots remain explicitly unfrozen.
- [x] Current README and website prose describe `1.0.0` as stable, and every hand-maintained current
      install example uses exactly `1.0.0`.
- [x] Historical What's New release sections remain unchanged; only the current top release section
      is rewritten as stable-v1 release prose, with no fictitious rc.24 history.
- [x] Public adoption prose contains no private product name, path, implementation detail,
      identifier, authorship metadata or CI metadata.
- [x] Focused sync-website tests, docs links, provenance, formatting and sync checks pass after
      integration with the stable release tree.
- [x] The complete local acceptance gate passes.

## Progress

- 2026-08-14: the consistency test failed first on the prerelease-only public and rustdoc policy.
  The implementation rewrites only current prose, keeps historical release sections intact, and
  adds a guard that identifies the offending source and line. The coordinator owns integrated
  focused checks, the release-wave gate and the final status transition.
- 2026-08-14: after integration with the stable version and the version-kind-aware guard, all 43
  synchronization tests pass, all 28 generated regions are current, the public content check is
  clean, and documentation links, provenance, Python compilation and whitespace checks pass.
- 2026-08-14: the integrated stable tree passed all 55 gate steps in 14m04s; this story closes.
