---
id: X-120
title: Run the WASM kernel checks in CI
pillar: Build
status: done
priority:
design:
epic: browser-sdk
areas: [ci, scripts, wasm]
predicate:
announcement:
note: check-wasm-kernel.sh exists and is green but nothing runs it · the artifact checks — no imports, export names, size bound — are unenforced
---

# Run the WASM kernel checks in CI

## Goal

Wire `scripts/check-wasm-kernel.sh` into CI and the gate, so the browser kernel's artifact
guarantees are enforced rather than merely available.

## Acceptance

- [x] A CI job runs `scripts/check-wasm-kernel.sh`, and `scripts/gate.py` carries it as a step or
      declares it in `NOT_RUN_LOCALLY` with a reason, so `gate.py --check` stays green.
- [x] The artifact checks are the point and must actually run: the module imports nothing, its
      export names match the ABI, and it stays inside its size bound. `cargo test -p sipx-wasm`
      already catches kernel regressions; these do not.
- [x] The job installs the `wasm32-wasip1` and `wasm32-unknown-unknown` targets and a wasm runtime
      explicitly rather than assuming a host that happens to have them.
- [x] A failing-first proof shows the job red when the module gains an import or an export is
      renamed.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `S-41`'s stated first risk. That story built the checker and left it
  unwired because adding a CI job needs a matching `scripts/gate.py` step in the same change, and
  `X-114` owned that file concurrently.

- 2026-08-08: implemented. `check-wasm-kernel.sh` is now gate step 42 **and** a `wasm` CI job —
  both halves in one change, because the drift check refuses one without the other, which is exactly
  what it caught when I added the step alone. **Local rather than CI-only**: it runs in ~18s warm,
  so a reason for excluding it would have been a preference dressed as a constraint. The CI job
  installs `wasm32-unknown-unknown`, `wasm32-wasip1` and `wasmtime` explicitly rather than assuming
  a host that happens to have them — the job's whole value is proving the kernel builds with no
  operating system under it, and an inherited toolchain would hide the failure it exists to catch.
  `rustup target add` joins `sudo` in `IGNORED_RUN_PREFIXES` as provisioning: mirroring it as a gate
  step would assert an installation rather than a property of the tree.
  Two guard tests in `test-gate.py`, proved failing-first by removing the step.

## Notes

- The `wasm/` package sits **outside** the workspace, like `fuzz/`, because `unsafe_code = "forbid"`
  refuses `#[unsafe(no_mangle)]`. A CI job therefore has to build it explicitly; `cargo
  --workspace` will not reach it.
- `S-41`'s corpus digest is pinned across targets. If `import-rfc4475-corpus.sh` gains a case the
  test says to re-derive the digest, not to edit it to match.
