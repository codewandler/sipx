---
id: X-62
title: An interop test failure is reported as peer success
pillar: Quality
status: done
priority: 2
areas: [interop]
announcement: 3
---

# An interop test failure is reported as peer success

## Goal

Make the interop runner's exit status preserve a failed role test, so the two-peer beta measurement
cannot print agreement after a test failed.

## Acceptance

- [x] A failing Cargo role test makes `tests/interop/run.sh` name the peer as failed and exit nonzero.
- [x] A failing-first lifecycle test drives the real copied runner with a failing Cargo stub.
- [x] Existing successful and concurrent-run behavior remains green.

## Progress

- Found while adding the WSS row for `P-13`: one peer's WSS registration timed out and Cargo exited
  nonzero, but `run_peer` continued out of its role loop, the outer runner printed `ok`, and the
  complete invocation exited zero. The proof therefore could not use the harness result until this
  exit propagation is repaired.
- `run_peer` now checks each Cargo invocation explicitly. This is intentionally not delegated to
  `set -e`: Bash disables errexit inside a function used as an `if` condition, which is exactly how
  the outer loop asks whether a peer passed. The copied-runner test failed first with exit zero and
  now observes the peer failure and nonzero runner exit.
