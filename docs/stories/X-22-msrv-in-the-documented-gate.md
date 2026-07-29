---
id: X-22
title: Put the MSRV check in the documented gate
pillar: Build
status: ready
priority: 3
design:
epic:
areas: [build, docs]
note: CI has an msrv job the gate does not name, so a green local gate lied through two releases
---

# Put the MSRV check in the documented gate

## Goal
`AGENTS.md`'s gate is the contract for "before marking any story done", and it is incomplete: CI
runs an `msrv` job that the gate never mentions. An implementor can run every documented command,
see green, and still break the build — which is not a hypothetical, it is what happened.

## Acceptance
- [ ] `AGENTS.md`'s gate block names the MSRV check, with the toolchain version derived from the
      workspace `rust-version` rather than written twice.
- [ ] The gate's commands are runnable from one entry point, so "the gate" is a thing that can be
      executed rather than a list that has to be transcribed correctly. Whether that is a script,
      a `just`/`make` target or a cargo alias is this story's decision, recorded.
- [ ] That entry point runs the same set CI runs, and there is a check that fails when the two
      drift — a gate that omits a CI job is exactly the defect this story exists for, and it must
      not be able to recur silently.
- [ ] The missing toolchain is a clear failure, not a skip: if `1.88.0` is not installed the gate
      says so and how to install it, rather than passing.

## Progress
- Not started.

## Notes
- The defect that motivated this: `BinaryHeap::<T>::new` required `T: Ord` on 1.88 and does not on
  later toolchains, so an unbounded `impl<K, I> Default for TimerQueue<K, I>` compiled on the dev
  toolchain and failed CI. The `msrv` job was red from the v0.4.0 release run through v0.7.0 —
  **two published releases did not build on the MSRV they advertise.** Fixed in `f761878`; this
  story is about why nobody noticed for five days.
- Related in kind, not in code: `check-features.sh` exists because `--all-features` hides breakage.
  Same lesson, different axis — the gate is only as good as the configurations it actually builds.
- `docs/compliance.md` and `rfc-report.py --check` are the house pattern for "a claim that cannot
  quietly lag its source". The drift check above should read like a sibling of that.
