---
id: A-10
title: Publish the stable crate set and diagnostic CLI artifacts
pillar: Application
status: in-progress
priority: 13
design: docs/specs/release-artifacts.md
epic: app-sdk
areas: [release, docs, sipx-cli]
note: promote the public beta only after every v1 predicate; stable archives and SBOM live here
---

# Publish the stable crate set and diagnostic CLI artifacts

## Goal

Turn a source checkout into versioned dependencies and diagnostic binaries that another project can
pin and reproduce.

## Acceptance

- [ ] Every v1 predicate in `docs/roadmap.md` is met; this story does not waive external-use or
      interop evidence to meet a date.
- [ ] All publishable crates pass `cargo publish --dry-run` and are published in dependency order
      with one workspace version and matching API documentation.
- [ ] Linux x86_64 and arm64 CLI archives are built once from the release commit, smoke-tested by
      digest and published with checksums and an SBOM.
- [ ] macOS and Windows compile-check the device feature; no binary publication promise is made for
      them in this milestone.
- [ ] A clean consumer project builds from registry versions only, and the published CLI completes a
      bounded loopback call without a repository checkout.
- [ ] Release notes name experimental surfaces and every intentional omission; no capability is
      promoted solely because code exists below its public caller.

## Progress

- 2026-08-05: selected for the stable-release wave together with `P-14`. A predicate audit found
  concrete evidence for four of the five roadmap predicates: alpha integrity held across beta.2
  through beta.7; every maturity layer is bound to an application caller; `A-9` records the public
  contract shaping an otherwise breaking enum change; and the diagnostic-phone proof requires two
  independent profiles for UDP, TCP, TLS, WS and WSS. No qualifying external application is yet
  recorded. Registry reverse dependencies currently resolve only to this workspace's own packages,
  so predicate 3 remains open rather than being inferred from downloads or examples.
- 2026-08-05: `docs/specs/release-artifacts.md` now fixes the five native target artifacts,
  no-optional-feature policy, static-musl proof, bounded call smoke test, SPDX identity, checksum
  aggregation and idempotent publication contract before workflow implementation.
- 2026-08-05: the tag workflow now builds and natively exercises the five-target matrix on exact
  release inputs, aggregates the ten published target files plus `SHA256SUMS`, creates stable or
  prerelease records from the version, and refuses to overwrite different existing bytes. The
  portable contract has 18 passing adversarial tests; its supervisor also completed a real call
  through the no-default-feature `sipx` binary. Stable publication remains blocked on roadmap
  predicate 3 and the actual release run, not on artifact implementation.
- 2026-08-05: `1.0.0-rc.1` is the first selected live exercise of that portable publication path.
  A successful candidate run can prove the mechanism and artifact bytes, but it cannot close this
  stable story while the independent-application predicate remains open.
