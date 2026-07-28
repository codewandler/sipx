---
id: X-1
title: Scaffold the Cargo workspace, lint policy, licensing and CI
pillar: Core
status: done
priority:
design:
epic:
areas: [build]
note:
---

# Scaffold the Cargo workspace, lint policy, licensing and CI

## Goal
Stand up the repository so every later story starts from a green, opinionated baseline: ten
crates that compile, lints that encode the project's principles, and CI that enforces them.

## Acceptance
- [x] Workspace with `crates/sipx-{sip,sdp,rtp,audio,media,transport,ua,call,cli,testkit}`,
      all compiling under `cargo check --workspace --all-targets`.
- [x] `sipx-sip` and `sipx-sdp` depend on no async runtime (enforced by review; a
      dependency-direction test follows in a later story).
- [x] Shared `[workspace.lints]`: `unsafe_code = "forbid"`, clippy `pedantic`, and
      `unwrap_used`/`expect_used`/`panic`/`indexing_slicing` as warnings.
- [x] Dual-licensed `MIT OR Apache-2.0` with both license texts present.
- [x] CI runs fmt, clippy `-D warnings`, tests, an MSRV check, `cargo-deny` and the
      provenance gate.
- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings` is clean.

## Progress
- Done. Workspace, `deny.toml`, `.github/workflows/ci.yml`, README, licenses all in place.
- MSRV set to 1.85 (edition 2024). No `rust-toolchain.toml`: pinning a channel would force a
  toolchain download on machines using a distribution Rust.

## Notes
- `anyhow` is a declared but currently unused dependency of `sipx-cli`; it lands with `P-1`.
