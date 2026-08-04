---
id: X-90
title: Make the release dependency policy pass
pillar: Build
status: done
priority: 1
design: docs/specs/release-rehearsal.md
epic: conformance
areas: [release, ci]
predicate: 4
announcement:
note: beta-1 blocker · exact-sha CI exposed a data-license exception and intentional path-only test edges
---

# Make the release dependency policy pass

## Goal

Restore the exact release commit's dependency-policy job without weakening the registry dependency
rules or making unpublished test support part of a public package.

## Acceptance

- [x] The failed `cargo-deny` job from exact commit `2794bee` is recorded as the failing-first
      observation: `webpki-roots 1.0.9` needs a reviewed data-license exception, and the five
      path-only `sipx-testkit` dev dependencies are the only wildcard findings.
- [x] `CDLA-Permissive-2.0` is allowed only for the exact `webpki-roots` package reviewed here, with
      the redistribution boundary recorded beside the exception; it is not admitted globally.
- [x] Path-only dependencies are exempt from the wildcard-version rule while registry and Git
      dependencies remain denied, preserving the release helper's unpublished-testkit boundary.
- [x] The same cargo-deny version used by CI passes locally, the complete local gate passes, and a
      new exact-sha GitHub CI run—including `cargo-deny` and the Pages deployment—finishes green.

## Progress

- GitHub Actions run `30903661581`, job `91973610369`, failed before any tag or registry write.
  Its six errors were one rejected `CDLA-Permissive-2.0` license and five wildcard findings, each
  naming a path-only `sipx-testkit` dev dependency. Advisories and sources passed.
- `cargo-deny 0.20.2`, the version pinned by the CI action, passes all four policy classes locally.
  Exact-sha run `30904773713` proves job `91977186078` green. That run remains red overall because
  the feature-matrix runner lacked the Linux device-audio build prerequisite tracked by `X-91`.
- The complete 32-step gate passed at `f30ffd2`. Exact-sha GitHub run `30905359518` then completed
  green, including cargo-deny job `91979064185` and Pages deployment job `91979625126`.

## Notes

- `allow-wildcard-paths` is the policy tool's narrow setting for this case; changing `wildcards`
  away from `deny` would also admit unbounded registry requirements and is not acceptable.
- The data license requires its text and warranty/liability disclaimers to accompany redistributed
  data. Cargo packages carry the dependency by reference; binary distributors remain responsible
  for the dependency notices that apply to their artifact.
