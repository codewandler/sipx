---
id: X-125
title: See what only the non-Linux jobs compile
pillar: Build
status: ready
priority: 30
design:
epic: conformance
areas: [ci, sipx-cli]
predicate:
announcement:
note: two CI jobs were red for a day over a cfg that disagreed with its only caller · the Linux gate compiles the caller, so it cannot see it
---

# See what only the non-Linux jobs compile

## Goal

Give the local gate a way to catch the defects that only appear on a platform it cannot build, so
`device audio compiles (macos-15)` and `(windows-2025)` stop being the first place anyone hears
about them.

## Why

`crates/sipx-cli/tests/support/strict_json.rs`'s `versioned_bytes` was gated
`#[cfg(feature = "device-audio")]`. Its only caller —
`dph_12_wav_and_virtual_device_carry_the_same_clip` — is gated
`#[cfg(all(feature = "device-audio", target_os = "linux"))]`. On Linux the caller compiles and the
helper is used, so `./scripts/gate.py` was green, `scripts/check-features.sh` was green including
its `sipx-cli device-audio` row, and every local signal said the tree was clean. On macOS and
Windows the caller is compiled out, the helper becomes dead code, and `-D warnings` makes that an
error. Both jobs were red across every push for a day, and the failure reads as a platform problem
while being a `cfg` that disagrees with its caller.

`NOT_RUN_LOCALLY` already records `device-portable` as unrunnable on a Linux gate host, and that is
honest for anything needing the macOS or Windows platform audio SDKs. It is *not* honest for this
class: nothing about a `cfg` disagreeing with its caller needs a platform SDK to notice.

Cross-compiling is not the obvious escape. `cargo check --target x86_64-pc-windows-msvc` was tried
on the gate host and fails in `ring`'s build script for want of a cross C toolchain, long before it
reaches any sipx code.

## Acceptance

- [ ] A check the gate can run on a Linux host reports a `cfg`-gated item whose gate is broader than
      the union of its callers' gates, for at least `crates/sipx-cli/tests/support/`, and fails on
      the `versioned_bytes` shape as it stood.
- [ ] A failing-first fixture reproduces exactly that shape — helper gated on the feature, sole
      caller gated on the feature *and* `target_os` — and the check reports it.
- [ ] **The check states its own scope in its header and in its output**, and asserts it scanned
      something. A guard that silently covers one directory, or that quietly matches nothing, is the
      failure mode this repository has shipped three times; whatever this cannot see must be written
      down rather than implied by a green line.
- [ ] Whether a cross-target `cargo check` is reachable for any non-Linux target is settled with a
      measurement rather than an assumption — if one is, it becomes a gate step and this story says
      what it costs; if none is, the reason is recorded beside `NOT_RUN_LOCALLY`'s entry so the next
      person does not re-derive it.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed while auditing why the last three `main` CI runs were red. The immediate defect
  is fixed in the same commit that files this — `versioned_bytes` now carries its caller's full
  `cfg`, `target_os` included — so the two jobs should go green without waiting for this story. What
  is left here is the blind spot, not the instance.
  The other red job in that run, `coverage`, was a different thing and is **already fixed on
  `main`**: `crates/sipx-call/tests/cancel.rs` used `#[expect(...)]` for three lints whose firing
  depends on coverage instrumentation, so the expectation was unfulfilled under nightly
  `cargo llvm-cov` and `-D warnings` rejected it. Commit `9b07c72` changed it to `#[allow(...)]`
  with the reason recorded at the site. Same family — a lint outcome that differs on a toolchain the
  gate does not run — and worth weighing when choosing this check's shape.
