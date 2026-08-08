---
id: X-125
title: See what only the non-Linux jobs compile
pillar: Build
status: in-progress
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

- [x] A check the gate can run on a Linux host reports a `cfg`-gated item whose gate is broader than
      the union of its callers' gates, for at least `crates/sipx-cli/tests/support/`, and fails on
      the `versioned_bytes` shape as it stood.
- [x] A failing-first fixture reproduces exactly that shape — helper gated on the feature, sole
      caller gated on the feature *and* `target_os` — and the check reports it.
- [x] **The check states its own scope in its header and in its output**, and asserts it scanned
      something. A guard that silently covers one directory, or that quietly matches nothing, is the
      failure mode this repository has shipped three times; whatever this cannot see must be written
      down rather than implied by a green line.
- [x] Whether a cross-target `cargo check` is reachable for any non-Linux target is settled with a
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

- 2026-08-08: implemented. Two guards, deliberately overlapping, because they fail in opposite
  directions.

  **`scripts/check-cfg-callers.py`** reads the shape without building it: for every `cfg`-gated item
  it computes the atoms required by the item and by each use of it, and reports an atom required by
  *every* use that the item does not require. That atom names a configuration in which the item
  compiles and no caller does. `all` requires everything, `any` requires only what all its branches
  agree on, `not(a)` is its own atom, and `target_os = "linux"` implies `unix` so that a correctly
  *narrower* item is not reported for spelling its platform differently.

  Failing-first, at merge base `2d2f576` with the checker moved out of the tree — `python3
  scripts/test-cfg-callers.py` → `FAILED (failures=28, errors=1)`, the named row reading
  `AssertionError: 1 != 2 : \`versioned_bytes\` was accepted: can't open file
  '.../scripts/check-cfg-callers.py'`. And against the real file with its pre-fix gate restored by
  hand, the finished checker prints:

      crates/sipx-cli/tests/support/strict_json.rs:125: `versioned_bytes` is gated
      `cfg(feature = "device-audio")`, and every one of its 1 use sites also requires
      `target_os = "linux"`.
        used at crates/sipx-cli/tests/cli.rs:1607, which requires feature = "device-audio",
        target_os = "linux"

  On the tree as it stands: 409 Rust files, 599 `cfg` attributes, 267 gated items with a use, none
  outliving its callers. It refuses to report a pass on fewer than 20 files, 40 attributes or 5
  examined items, and prints its scope and its blind spots on green runs as well as red ones.

  Three shapes in this workspace made it silent while it was being written, and each is now a
  fixture: a `cfg` on a *function parameter* (`sipx-transport`'s `dial_ws`) read as a `cfg` on the
  body; a multi-line `#[allow(…)]` between a `cfg` and its item (`cli.rs`) whose second line was
  taken for the subject; and `}}` inside a `format!` string (`cli.rs`'s ALSA configuration) counted
  as structure, which closed the enclosing test early and left the caller of `versioned_bytes`
  looking ungated. The first of those produced a *false* report on correct code.

  **The cross-target measurement**, settled rather than assumed, on 2026-08-08:

  | target | result |
  |---|---|
  | `x86_64-pc-windows-gnu` | **builds.** `cargo check -p sipx-cli --all-targets --features device-audio --target x86_64-pc-windows-gnu` succeeds; 23 s from a cold worktree with a warm sccache, 2 s fully warm |
  | `x86_64-apple-darwin` | fails in `ring`'s build script — `cc: error: unrecognized command-line option '-arch'`, before any sipx code |
  | `aarch64-apple-darwin` | same, with `-mmacosx-version-min=11.0` |
  | `x86_64-pc-windows-msvc` | fails in `ring`'s build script — `failed to find tool "lib.exe"` (this is what `## Why` recorded) |

  So one is reachable and it is not the one CI runs. `x86_64-pc-windows-gnu` compiles every
  `target_os = "windows"`, `windows` and `target_family = "windows"` branch that `windows-2025`
  does, which is the whole of this defect class; the MSVC environment, linking and the platform
  audio SDK stay CI's. It is now the `windows cross check` gate step against a new `cross-windows`
  CI job. **What it costs:** the `x86_64-pc-windows-gnu` rustup target and a mingw-w64 C toolchain
  for `ring`'s build script — the CI job installs both, `AGENTS.md`'s gate section names them, and a
  host without them gets a cargo failure rather than a silent skip. Proof it catches the instance:
  with the pre-fix gate restored, that exact command exits 101 with
  `error: function \`versioned_bytes\` is never used … \`-D dead-code\` implied by \`-D warnings\``.

  The `macos-15` half has no local counterpart and now says so where the next person will look:
  `gate.py`'s `NOT_RUN_LOCALLY["device-portable"]` carries the measurement instead of the old
  reason, which spoke about audio SDKs and was true of linking but false of the `cfg` disagreement
  that actually went red.

  Owed to the changelog when this closes, since that file is the coordinator's: *"The gate now
  reports a `cfg`-gated item whose gate is broader than the union of its callers' gates — dead code
  on a platform a Linux host cannot build — and cross-checks `sipx-cli` for
  `x86_64-pc-windows-gnu`, the one non-Linux target this workspace builds from Linux."*

  Left open: the `gate` row. Per the dispatch, the full `./scripts/gate.py` was not run here — one
  gate runs per wave. Everything else was: the checker directly, its suite (29 tests),
  `./scripts/gate.py --check`, `python3 scripts/test-gate.py` (110 tests), `cargo fmt --all --check`
  and the new cross-target step's own command.
