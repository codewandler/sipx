---
id: P-14
title: Ship portable CLI artifacts
pillar: Phone
status: backlog
priority: 16
design: docs/designs/demand.md
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

- [ ] Release artifacts are published for, at minimum: `x86_64` and `aarch64` Linux against musl,
      `aarch64` and `x86_64` macOS, and `x86_64` Windows.
- [ ] The Linux musl artifacts are **statically linked and verified as such** by a check in CI —
      `ldd` reporting a dynamic loader on a binary claimed static fails the job. The reported field
      failure is a glibc version mismatch, which a dynamically linked "portable" binary reproduces.
- [ ] Optional C-dependent features are stated per artifact. If the Opus feature is enabled in a
      published binary, its linkage is static too or the artifact says it is not portable — silently
      shipping a binary that fails on the user's box is the defect.
- [ ] Each artifact is smoke-tested in CI on its target: run `sipx version` and one command that
      exercises the SIP stack, not merely that the binary executes.
- [ ] Checksums are published alongside, and the documented install path in
      `website/docs/getting-started.md` covers the binary download as well as `cargo install`.
- [ ] The release rehearsal (`A-11`) covers these artifacts, so distribution stays reproducible.
- [ ] `./scripts/gate.py` green; new CI jobs registered in `gate.py` or `NOT_RUN_LOCALLY` with a
      reason so `gate.py --check` stays green.

## Progress
- (not started)

## Notes
- The single most-engaged issue in the surveyed CLI project is users hitting `GLIBC_2.29`/`2.32`/
  `2.34` and `GLIBCXX` errors on a downloaded binary. Rust against musl makes this largely a
  build-matrix problem rather than an engineering one — a genuine structural advantage worth
  actually collecting.
- Sequence with `A-10` (publish the stable crate set and CLI artifacts); this story is the portability
  requirements on what that one publishes, and the two should not fight over the same workflow.
