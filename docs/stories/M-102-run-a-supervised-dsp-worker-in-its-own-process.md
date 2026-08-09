---
id: M-102
title: Run a supervised DSP worker in its own process
pillar: Media
status: done
priority: 39
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-media, dsp, realtime, m18]
predicate:
announcement:
note: after M-64 · the operating-system half of the supervised-isolated profile
---

# Run a supervised DSP worker in its own process

## Goal

Give the supervised-isolated execution profile the process boundary its containment claim is
written against, so a worker that corrupts its own memory or must be killed outright costs a
`SIGKILL` and a `wait` rather than the media process.

## Why this is separate from `M-64`

`M-64` implemented everything about the profile that is a *protocol*: the bounded request and
result channels, the deadline stated in frame durations, the miss budget, the declared action on a
deadline, a crash, a malformed result or a lost worker, and the terminate-and-reap step of the
teardown barrier. What it did not implement is the boundary those run across. Its worker runs in a
thread `sipx-media` owns, which is enough to keep `process` off the media worker and to bound the
wait, and is **not** enough for the two sentences
[`docs/specs/custom-call-dsp.md`](../specs/custom-call-dsp.md) §7.2 writes about an operating-system
process: that a worker's own memory and CPU are bounded by what the operating system is configured
to bound, and that a worker which will not stop can be terminated regardless.

A thread cannot be killed, so a supervised worker that never returns from one `run` call today
holds its thread until the process ends. It holds no media worker and stalls no RTP — that is the
containment `M-64` does deliver and prove — but it is a leak with a bound of "the life of the
process", and the profile's documentation must not be read as promising otherwise until this ships.

## Acceptance

- [x] A supervised worker runs in an operating-system process `sipx-media` spawns and owns, reached
      over a framed protocol specified in `docs/specs/call-dsp-graph.md` §7 before it is written.
- [x] The protocol carries direction, format, sample position, discontinuity and interleaved signed
      16-bit samples, and refuses a malformed or oversized message by type rather than by trusting a
      length prefix.
- [x] Spawning, writing and reading never happen on the media worker; the media worker's contact
      stays the two non-blocking channel operations `M-64` already defines.
- [x] Cancellation terminates the process and reaps it, including a worker that ignores a request to
      stop, and the teardown barrier reports zero workers only once `wait` has returned.
- [x] A worker that exits, crashes or is killed applies the declared failure action and is never
      restarted silently.
- [ ] Failing-first tests cover deadline, hang, crash, malformed result and reap against a real
      child process, and the full gate is green.

## Progress

- Backlog. Filed by `M-64` on 2026-08-09, which implemented the profile's channel discipline and
  failure policy and left the process boundary unimplemented and stated as unimplemented, in
  `crates/sipx-media/src/lib.rs`'s stability section and in
  [`docs/specs/call-dsp-graph.md`](../specs/call-dsp-graph.md) §7.

- 2026-08-09 — **the process boundary is in.** A supervised stage is now a *program*
  (`GraphPlan::with_supervised(WorkerProcess)`), spawned with `std::process::Command`, driven over
  the framed protocol written into [`docs/specs/call-dsp-graph.md`](../specs/call-dsp-graph.md)
  §7.4 before any of it was implemented, killed unconditionally at cancellation and `wait`ed for
  before the barrier reports zero workers (§7.3). The five rows above are ticked; the last one is
  not, because the full gate is the coordinator's to run.

  **Failing-first, at the merge base `98983cf`.** The named test was written against the
  thread-hosted door that existed there — a `SupervisedWorker` that records
  `std::process::id()` — and
  `cargo test -p sipx-media --test dsp_worker` reported:

  ```
  test a_supervised_worker_runs_in_its_own_operating_system_process ... FAILED
  assertion `left != right` failed: a supervised worker must run in an operating-system process of
  its own, not in the media process: `docs/specs/custom-call-dsp.md` §7.2's containment claim is
  written against a process that can be bounded and killed, and a thread is neither
    left: 440899
   right: 440899
  ```

  The same pid twice is the whole finding. The test now asserts the same thing through the process
  door — `DspGraph::worker_pids()` against `std::process::id()`, cross-checked against the pid the
  worker writes into the audio itself, so what is proven is that the DSP ran *there* and not merely
  that something was spawned.

  **Which of `M-64`'s two options.** Neither tokio's `process` feature nor a new dependency: a
  `[[bin]]` worker target, `sipx-dsp-worker`, plus `std::process` and two threads per stage. The
  tokio feature would not have been sufficient on its own — a child process needs a *program*, and
  the acceptance's "against a real child process" needs one a test can locate, which
  `CARGO_BIN_EXE_sipx-dsp-worker` is the only cargo-guaranteed way to do. Doing it with `std` keeps
  the media worker's channels exactly the `std::sync::mpsc::sync_channel` pair `M-64` proved
  allocation-free rather than migrating them to tokio's. **The cost accepted:** a published package
  now ships an executable, so `cargo install sipx-media` installs it and its command line is a
  compatibility surface. §7.5 states what it is for.

  **Public API, and it is a break.** `with_supervised` takes a `WorkerProcess` instead of a
  `Box<dyn SupervisedWorker>`; `SupervisedWorker` moved to the worker's side of the boundary and its
  `run` now takes a `DspFrame` — the wire carries position and discontinuity, so the worker sees
  them. `serve_worker`, `WorkerProcess`, `WorkerProtocolError` and `DspGraph::worker_pids` are new.
  `dsp` is documented **Experimental** in `crates/sipx-media/src/lib.rs`, which is the reservation
  this spends.

  **Owed to `CHANGELOG.md`** (fenced from this implementor, for the coordinator to write): *A
  supervised DSP stage now runs its worker in an operating-system process `sipx-media` spawns, kills
  and reaps, reached over the framed protocol of `docs/specs/call-dsp-graph.md` §7.4;
  `GraphPlan::with_supervised` takes a `WorkerProcess` naming a worker program instead of a boxed
  `SupervisedWorker`, which now runs in that program behind `sipx_media::dsp::serve_worker`, and the
  `sipx-dsp-worker` reference worker ships with the crate (`M-102`).*

  **Left open, deliberately.** The pump discovers a dead worker when it next writes or reads, which
  is the next frame of a live call — so a worker that dies while a call is idle is not reaped until
  the next frame or the teardown, whichever comes first. That is bounded and reported, not a leak,
  and it is why the tests wait for the process to be gone rather than for a duration.

- 2026-08-09: closed at the `1.0.0-rc.17` boundary, against the wave gate run on this tree.
