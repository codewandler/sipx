---
id: X-34
title: Make the gate fail honestly when the disk is full
pillar: Build
status: done
design: docs/designs/sip-core.md
epic: conformance
areas: [tests]
note: five times on 2026-07-29 a full disk produced a red gate that read as a code defect — cargo reports ENOENT on build artifacts, and a correct merge was nearly reverted for it
---

# Make the gate fail honestly when the disk is full

## Goal
Stop a full disk from looking like a broken diff. `./scripts/gate.py` should refuse to start, or stop
and say so plainly, rather than letting cargo report a missing file and leave a human to guess.

## Acceptance
- [x] The gate checks free space before it starts and **fails with a message naming disk**, not a
      compile error, when there is not enough for a cold build. The threshold is derived from a
      measured cold-build size rather than guessed, and stated in the message alongside the actual
      free space.
- [x] A step that dies of `ENOSPC` or of the ENOENT-on-artifact shape is **reported as an
      infrastructure failure, distinct from a red step**. This is the whole point: cargo's messages
      in this state are actively misleading. Real examples from one evening:
      `failed to create file '…/target/debug/examples/canned_program.d': No such file or directory
      (os error 2)`, `failed to write '…/.fingerprint/rand-…/invoked.timestamp'`, and
      `extern location for autocfg does not exist: …/libautocfg-….rlib`. Every one reads as a code
      error; all three were a vanished `target/`.
- [x] **Consider a shared `CARGO_TARGET_DIR` for concurrent worktrees**, and record the decision
      either way. Each worktree currently pays its own ~12–14 GB cold build, so three implementors
      plus an integration gate cannot coexist on this machine. Sharing trades parallelism for disk,
      because cargo locks the directory — that trade-off is the substance of this item, not an
      obvious win.
- [x] The check itself is tested, in `scripts/test-gate.py` beside the gate's other self-tests — a
      fake free-space reading below the threshold must make the gate refuse.
- [x] Failing-first test: the gate today starts and runs to a misleading failure with insufficient
      free space. Name the test that makes it refuse instead.

## Progress
- **Done.** `scripts/gate.py` grew a disk guard beside `--check`'s drift guard, on the same
  principle: a gate that cannot be believed should not report.
- **The threshold is measured.** A full gate run in a cold worktree was measured step by step on
  this machine: clippy 0.7 GiB, then `cargo test --workspace --all-features` +8.4 (it links every
  test binary — the expensive step by an order of magnitude), examples +0.0, `msrv` +0.6 (a second
  toolchain keeps its own artifacts), feature matrix +0.3, docs site +0.5. **10.6 GiB in total, all
  steps green.** That agrees with the figure taken from the other end — the integration worktree's
  `target/` grew 13 GiB → 22 GiB over one evening's runs, so a run costs about ten gigabytes there
  too. Threshold: 10.6 GiB + 10% for cargo's link-time peak = **11.7 GiB**, and both numbers are in
  the refusal message. The measurements live in `MEASURED_GATE_TARGET_GIB` with their provenance,
  and `test_the_threshold_covers_every_size_ever_measured` fails if the threshold ever drops below
  one of them.
- **A run cut short is a non-result, not a red gate.** ENOSPC and cargo's three ENOENT-on-artifact
  shapes end the run with `gate: NOT A RESULT — the machine stopped this run, not the tree`, the
  quoted line, why that line is not the reader's diff, the free space, and exit code **2**. A red
  step still prints `N of M steps failed` and exits 1. Steps already red when the disk gave out are
  kept, under a heading that claims nothing about them.
- The run **ends** at a disk failure rather than continuing: once `target/` is gone every remaining
  step fails for the same reason, and that wall of red is what misled five readers. A floor is
  re-checked between steps too, so a disk another worktree fills mid-run stops this one at the next
  boundary with a sentence instead of a red step.
- **Shared `CARGO_TARGET_DIR`: rejected**, argued in `scripts/gate.py` under "The disk guard" so
  the next person out of disk finds the reasoning rather than re-deriving it. Three grounds: cargo
  takes an exclusive lock on the build directory, so sharing converts the 3-plus-implementor
  fan-out into a queue and the fan-out is what makes the backlog move; it promotes one worktree's
  `cargo clean` — or its deletion, which is how implementor worktrees end — into everyone's
  vanished `target/`, which is occurrence 4 above, as a design feature rather than an accident; and
  the saving is smaller than it looks, because worktrees hold different code and only the
  dependency artifacts are genuinely shared, while the part that shares without a lock
  (`CARGO_HOME`) already does. `CARGO_TARGET_DIR` is still honoured if a caller sets it — the
  decision is that the gate does not set it, not that it argues with someone who has.
- **Crediting an existing `target/` against the threshold was tried and dropped.** It was in the
  failing test first, to stop the guard from refusing warm runs that would have succeeded. The
  13 GiB → 22 GiB observation killed it: a warm `target/` is no evidence that the expensive part
  was ever built, so the credit would have let exactly that run start with 2 GiB free. With the
  threshold measured at one run's cost rather than at an accumulated `target/` size, a flat
  requirement is not onerous and needs no credit.
- Steps now stream their output through the gate process, because classifying a failure means
  reading what it said. Cargo keeps its colour (`ci.yml`'s `env:` sets `CARGO_TERM_COLOR: always`);
  a shell step that tests for a tty itself will print plainly.

## Notes
- **Five occurrences in one evening (2026-07-29)**, which is why this is priority 3 and not a
  nice-to-have:
  1. `X-30`'s integration gate: 5 of 18 steps red, every one disk, none the diff.
  2. `X-19`'s integration gate: reported red, diagnosed as disk, green unchanged on re-run.
  3. `X-28`'s integration gate: 3 steps red — `msrv` and the app contract passed untouched on
     re-run, and the one real failure was in a crate the diff never opened.
  4. Two implementor runs destroyed outright: one had its `target/.fingerprint` vanish underneath
     cargo mid-gate, then its whole worktree.
  5. The `0.10.0` release gate, red on 5 steps at 0 bytes free; green after `cargo clean`.
- **The cost is not the wasted minutes, it is the near-miss.** `X-28`'s merge was one command away
  from being reverted for a defect in `sipx-transport` that its diff never touched. That is exactly
  what `X-28` itself was filed to prevent — *a test that fails at random trains everyone to re-run
  the gate instead of reading it* — one layer down: a **gate** that fails at random trains everyone
  to re-run it instead of believing it.
- Reads with `X-29`: same disease, different layer. `X-29` is tests that fail because the machine is
  busy; this is the gate failing because the machine is full. Both end in a red signal that means
  nothing, which is the one thing this project's whole discipline rests on not happening.
- Raised independently by `S-26`'s implementor at handoff, having lost its own worktree to it: *"the
  gate has no disk-space guard, and cargo's failure mode is a misleading ENOENT that looks like a
  code error."*
- **The gate already checks itself** against `ci.yml` (`X-22`) and refuses to drift. Refusing to run
  when it cannot produce a trustworthy answer is the same principle: a gate that cannot be believed
  should not report.
