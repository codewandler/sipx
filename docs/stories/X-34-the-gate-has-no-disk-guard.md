---
id: X-34
title: Make the gate fail honestly when the disk is full
pillar: Build
status: ready
priority: 3
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
- [ ] The gate checks free space before it starts and **fails with a message naming disk**, not a
      compile error, when there is not enough for a cold build. The threshold is derived from a
      measured cold-build size rather than guessed, and stated in the message alongside the actual
      free space.
- [ ] A step that dies of `ENOSPC` or of the ENOENT-on-artifact shape is **reported as an
      infrastructure failure, distinct from a red step**. This is the whole point: cargo's messages
      in this state are actively misleading. Real examples from one evening:
      `failed to create file '…/target/debug/examples/canned_program.d': No such file or directory
      (os error 2)`, `failed to write '…/.fingerprint/rand-…/invoked.timestamp'`, and
      `extern location for autocfg does not exist: …/libautocfg-….rlib`. Every one reads as a code
      error; all three were a vanished `target/`.
- [ ] **Consider a shared `CARGO_TARGET_DIR` for concurrent worktrees**, and record the decision
      either way. Each worktree currently pays its own ~12–14 GB cold build, so three implementors
      plus an integration gate cannot coexist on this machine. Sharing trades parallelism for disk,
      because cargo locks the directory — that trade-off is the substance of this item, not an
      obvious win.
- [ ] The check itself is tested, in `scripts/test-gate.py` beside the gate's other self-tests — a
      fake free-space reading below the threshold must make the gate refuse.
- [ ] Failing-first test: the gate today starts and runs to a misleading failure with insufficient
      free space. Name the test that makes it refuse instead.

## Progress
- Not started.

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
