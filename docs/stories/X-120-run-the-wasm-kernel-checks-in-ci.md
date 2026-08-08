---
id: X-120
title: Run the WASM kernel checks in CI
pillar: Build
status: ready
priority: 7
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

- [ ] A CI job runs `scripts/check-wasm-kernel.sh`, and `scripts/gate.py` carries it as a step or
      declares it in `NOT_RUN_LOCALLY` with a reason, so `gate.py --check` stays green.
- [ ] The artifact checks are the point and must actually run: the module imports nothing, its
      export names match the ABI, and it stays inside its size bound. `cargo test -p sipx-wasm`
      already catches kernel regressions; these do not.
- [ ] The job installs the `wasm32-wasip1` and `wasm32-unknown-unknown` targets and a wasm runtime
      explicitly rather than assuming a host that happens to have them.
- [ ] A failing-first proof shows the job red when the module gains an import or an export is
      renamed.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `S-41`'s stated first risk. That story built the checker and left it
  unwired because adding a CI job needs a matching `scripts/gate.py` step in the same change, and
  `X-114` owned that file concurrently.

## Notes

- The `wasm/` package sits **outside** the workspace, like `fuzz/`, because `unsafe_code = "forbid"`
  refuses `#[unsafe(no_mangle)]`. A CI job therefore has to build it explicitly; `cargo
  --workspace` will not reach it.
- `S-41`'s corpus digest is pinned across targets. If `import-rfc4475-corpus.sh` gains a case the
  test says to re-derive the digest, not to edit it to match.
