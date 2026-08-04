---
id: A-10
title: Publish the stable crate set and diagnostic CLI artifacts
pillar: Application
status: backlog
priority: 13
design: docs/roadmap.md
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

- Not started.
