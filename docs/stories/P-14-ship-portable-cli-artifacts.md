---
id: P-14
title: Ship portable CLI artifacts
pillar: Phone
status: in-progress
priority: 16
design: docs/specs/release-artifacts.md
epic: demand
areas: [ci, sipx-cli]
predicate:
announcement:
note: the surveyed stack's most-engaged issue is a glibc failure · Rust makes this a build-matrix problem
---

# Ship portable CLI artifacts

## Goal

Publish `sipx` binaries that run on the machines people have, without a runtime library hunt.

## Acceptance

- [x] Release artifacts are published for, at minimum: `x86_64` and `aarch64` Linux against musl,
      `aarch64` and `x86_64` macOS, and `x86_64` Windows.
- [x] The Linux musl artifacts are **statically linked and verified as such** by a check in CI —
      `ldd` reporting a dynamic loader on a binary claimed static fails the job. The reported field
      failure is a glibc version mismatch, which a dynamically linked "portable" binary reproduces.
- [x] Optional C-dependent features are stated per artifact. If the Opus feature is enabled in a
      published binary, its linkage is static too or the artifact says it is not portable — silently
      shipping a binary that fails on the user's box is the defect.
- [x] Each artifact is smoke-tested in CI on its target: run `sipx version` and one command that
      exercises the SIP stack, not merely that the binary executes.
- [x] Checksums are published alongside, and the documented install path in
      `website/docs/getting-started.md` covers the binary download as well as `cargo install`.
- [x] The release rehearsal (`A-11`) covers these artifacts, so distribution stays reproducible.
- [ ] `./scripts/gate.py` green; new CI jobs registered in `gate.py` or `NOT_RUN_LOCALLY` with a
      reason so `gate.py --check` stays green.

## Progress
- 2026-08-05: selected with `A-10`; `docs/specs/release-artifacts.md` defines the exact five-target
  archive matrix, static-musl evidence, feature disclosure, native loopback smoke proof, SPDX 2.3
  documents, checksum aggregation and retry behavior before the build workflow is changed.
- 2026-08-05: implemented the native matrix, deterministic archives, static ELF inspection,
  target-filtered SPDX closure, exact-set aggregation and retry byte comparison. All 18 local
  contract tests, the 33 workflow mutation tests, a real no-default-feature loopback call, the
  release-workflow check, CI/gate drift check and public-site build pass. The story stays
  in-progress until a tag run publishes and natively records all five artifacts.
- 2026-08-05: `1.0.0-rc.2` is selected as the first published tag that must verify the complete
  five-target set. The public install guide now covers those prerelease archives instead of
  presenting them only as a future stable-release path.

- 2026-08-08: **the `v1.0.0-rc.2` tag run published and natively recorded the complete five-target
  set**, which was the condition this story was held open for. Protected run `31052427439` succeeded
  2026-08-05T22:21:21Z; release
  <https://github.com/codewandler/sipx/releases/tag/v1.0.0-rc.2> carries
  `aarch64-apple-darwin`, `aarch64-unknown-linux-musl`, `x86_64-apple-darwin`,
  `x86_64-pc-windows-msvc` and `x86_64-unknown-linux-musl`, each with its archive and SPDX 2.3
  document, alongside one aggregated `SHA256SUMS`. Only the shared repository-gate row stays open.

## Notes
- The single most-engaged issue in the surveyed CLI project is users hitting `GLIBC_2.29`/`2.32`/
  `2.34` and `GLIBCXX` errors on a downloaded binary. Rust against musl makes this largely a
  build-matrix problem rather than an engineering one — a genuine structural advantage worth
  actually collecting.
- Sequence with `A-10` (publish the stable crate set and CLI artifacts); this story is the portability
  requirements on what that one publishes, and the two should not fight over the same workflow.
