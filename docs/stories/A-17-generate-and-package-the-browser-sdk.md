---
id: A-17
title: Generate and package the browser SDK
pillar: Application
status: backlog
priority: 17
design: docs/designs/browser-sdk.md
epic: browser-sdk
areas: [browser, javascript, typescript, wasm, npm, m15]
predicate:
announcement:
note: after S-41, T-33 and M-52 · generated ABI types plus small handwritten ergonomic layer
---

# Generate and package the browser SDK

## Goal

Produce an installable JavaScript package with checked TypeScript declarations around the WASM,
signalling and audio adapters, without duplicating the Rust protocol vocabulary by hand.

## Acceptance

- [ ] ABI values, events, commands and errors generate JavaScript glue and TypeScript declarations
      from one checked source; a drift test fails when Rust and the package disagree.
- [ ] A small handwritten layer exposes register, dial, answer, hangup and event subscription with
      explicit cancellation and adds no protocol state hidden from the kernel.
- [ ] The package supports the browser module/bundler targets selected by A-16, ships the WASM asset,
      has no Node runtime dependency, and declares its browser and feature requirements.
- [ ] Package exports do not expose credentials, raw private key material or unsafe generic message
      mutation; errors remain typed across the WASM boundary.
- [ ] A clean temporary JavaScript consumer installs the packed artifact, type-checks and builds with
      no workspace path dependency.
- [ ] Reproducible package contents, license files, provenance metadata and checksums are checked in
      CI; this story does not publish externally without separate release authorization.
- [ ] The full gate is green.

## Progress

- Backlog. Depends on S-41, T-33 and M-52.
