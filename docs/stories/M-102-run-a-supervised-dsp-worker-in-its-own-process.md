---
id: M-102
title: Run a supervised DSP worker in its own process
pillar: Media
status: backlog
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

- [ ] A supervised worker runs in an operating-system process `sipx-media` spawns and owns, reached
      over a framed protocol specified in `docs/specs/call-dsp-graph.md` §7 before it is written.
- [ ] The protocol carries direction, format, sample position, discontinuity and interleaved signed
      16-bit samples, and refuses a malformed or oversized message by type rather than by trusting a
      length prefix.
- [ ] Spawning, writing and reading never happen on the media worker; the media worker's contact
      stays the two non-blocking channel operations `M-64` already defines.
- [ ] Cancellation terminates the process and reaps it, including a worker that ignores a request to
      stop, and the teardown barrier reports zero workers only once `wait` has returned.
- [ ] A worker that exits, crashes or is killed applies the declared failure action and is never
      restarted silently.
- [ ] Failing-first tests cover deadline, hang, crash, malformed result and reap against a real
      child process, and the full gate is green.

## Progress

- Backlog. Filed by `M-64` on 2026-08-09, which implemented the profile's channel discipline and
  failure policy and left the process boundary unimplemented and stated as unimplemented, in
  `crates/sipx-media/src/lib.rs`'s stability section and in
  [`docs/specs/call-dsp-graph.md`](../specs/call-dsp-graph.md) §7.
