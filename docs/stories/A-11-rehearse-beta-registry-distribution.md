---
id: A-11
title: Rehearse registry distribution for the public beta
pillar: Application
status: done
priority: 10
design: docs/specs/release-rehearsal.md
epic: app-sdk
areas: [release, docs, sipx-cli]
predicate:
announcement: 4
note: package every public crate and prove the release procedure without publishing
---

# Rehearse registry distribution for the public beta

## Goal

Turn the workspace into a reproducible, side-effect-free beta publication rehearsal before any
credential can publish a crate.

## Acceptance

- [x] A release helper derives the public package dependency order from Cargo metadata, checks one
      workspace version and refuses a dirty or mismatched release checkout.
- [x] Every public package at `1.0.0-beta.1` passes the locked Cargo workspace publication dry-run
      from the clean release candidate; `sipx-testkit` remains unpublished.
- [x] The helper's default and CI modes cannot create a tag, publish a package or contact an
      announcement channel; those actions require an explicit publish mode and confirmation.
- [x] Package contents include the declared README/license metadata and no path or Git dependency
      escapes into the published manifest.
- [x] Tests exercise ordering, version mismatch, partial availability and refusal to publish from
      anything except the exact clean release tag.

## Progress

- The release contract, helper and thirty-nine adversarial fixtures are implemented. The generic load
  scheduler now lives on the published `sipx-call` surface, and public manifests keep unpublished
  `sipx-testkit` test support path-only so Cargo removes it while normalizing registry manifests. A
  real dirty-tree inspection built all eleven public archives together, matched every
  `cargo package --list`, and verified their normalized README, license and dependency metadata.
  The formerly blocked `sipx-sip` package also completed Cargo's archive build verification from
  the dirty candidate. Default `check` now refuses only because the shared checkout is dirty, as
  required. A disposable clean shadow candidate copied the current tree, changed only the workspace,
  lockfile and one explicit internal requirement from `1.0.0-alpha.5` to `1.0.0-beta.1`, and made a
  local snapshot outside the user's repository. From that candidate, the helper's locked workspace
  publication dry-run packaged and compiled all eleven public beta packages through Cargo's
  temporary registry, reached every dry-run upload boundary, and left `sipx-testkit` unpublished.
  No tag, registry write or change to the user's repository was made. Publication probes now fail
  closed on registry errors, and a partial release cannot advance until freshly reproduced tagged
  archives match the canonical crates.io checksums for every already-visible package.
- After the Opus package and public-documentation work settled, the disposable clean
  `1.0.0-beta.1` shadow was rebuilt and the exact locked workspace publication dry-run again
  packaged, compiled and reached the dry-run upload boundary for all eleven public packages. The
  full 30-step project gate then passed on the combined source tree.

## Notes

- The Rust entry points remain modular: `sipx-call` for endpoints and `sipx-sip` for sans-I/O use.
- Linux archives, checksums and an SBOM remain A-10's stable-release promise, not a beta blocker.
