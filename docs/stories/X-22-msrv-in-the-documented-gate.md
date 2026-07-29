---
id: X-22
title: Put the MSRV check in the documented gate
pillar: Build
status: done
priority:
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
- [x] `AGENTS.md`'s gate block names the MSRV check, with the toolchain version derived from the
      workspace `rust-version` rather than written twice.
- [x] The gate's commands are runnable from one entry point, so "the gate" is a thing that can be
      executed rather than a list that has to be transcribed correctly. Whether that is a script,
      a `just`/`make` target or a cargo alias is this story's decision, recorded.
- [x] That entry point runs the same set CI runs, and there is a check that fails when the two
      drift — a gate that omits a CI job is exactly the defect this story exists for, and it must
      not be able to recur silently.
- [x] The missing toolchain is a clear failure, not a skip: if `1.88.0` is not installed the gate
      says so and how to install it, rather than passing.

## Progress
- The entry point is `scripts/gate.py`. **The decision, recorded:** a `just`/`make` target would be
  a second list in a second syntax that can read neither `Cargo.toml` nor `ci.yml`, and a cargo
  alias cannot run a shell script — half the gate is shell scripts. Python keeps the step list, the
  drift check and the MSRV derivation in one file, which is what makes the drift check worth
  having: there is nowhere for a step to exist unchecked.
- `--check` is the drift check, and it reads `.github/workflows/ci.yml` rather than restating it.
  Four claims: every command a CI job runs is a gate step or is in `NOT_RUN_LOCALLY` with a reason;
  every gate step names a job that runs it; a flag CI passes and the step drops is drift unless the
  step declares it with a reason (`check-provenance.sh --history` is the one real difference); and
  the `msrv` job's `dtolnay/rust-toolchain@` pin equals the workspace `rust-version`.
- `AGENTS.md`'s gate block may invoke the entry point and nothing else — checked, so the section
  cannot grow its own copy of the list. The MSRV version appears in neither the block nor the
  script; `version_literal_problems` fails if it does.
- The MSRV toolchain is derived from `rust-version` and padded to three components. Absent
  toolchain (or absent rustup) is a failed step printing `rustup toolchain install <version>`,
  never a skip.
- New CI job `gate` runs `gate.py --check`, `test-gate.py` and `test-rfc-report.py`. Judgement call
  taken: **yes**, `test-rfc-report.py` belongs here — X-15 wrote it and nothing ran it, and it is
  milliseconds with no dependency beyond a Python interpreter.
- Two other omissions the check surfaced on the way: the documented gate never ran the `test` job's
  `cargo build --examples`, and it ran without CI's `RUSTFLAGS: -D warnings`. Both are gate steps
  now, and the environment is read from `ci.yml`'s `env:` block rather than repeated.

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
