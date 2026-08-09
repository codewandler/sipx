---
id: X-143
title: Record the DSP cost run on a quiet box
pillar: Build
status: backlog
priority: 42
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [dsp, benchmark, documentation, m18]
predicate:
announcement:
note: after X-109 · the harness exists and refuses to report under load; the figure is untaken
---

# Record the DSP cost run on a quiet box

## Goal

Take the per-processor CPU figure `X-109` built the harness for, on a machine quiet enough for it to
mean something, and record it where a later change can be compared against it.

## Context

`X-109` measured everything about the call-DSP processors that integer arithmetic can measure —
frequency response exactly, heap exactly — and **deliberately recorded no CPU figure**, because CPU
is a clock reading and the machine it ran on was shared with four other implementors.

The harness is `crates/sipx-audio/examples/dsp_cost.rs` and it is finished. It reports nanoseconds
per thousand positions and parts per million of one core at 8,000 Hz, per processor, over
`docs/specs/call-dsp-noise-reduction.md` §8's four conditions at their exact lengths. It refuses
three ways — a debug build, a one-minute load average above a tenth of the machine's cores, and its
own control workload drifting more than 10% between the start of the run and the end — and on
`X-109`'s machine the second and third both fired. `docs/specs/call-dsp-effects.md` §10.4 and
`call-dsp-noise-reduction.md` §8.1 both say the row is empty and why.

What is left is one run on an idle box and a page around it. Nothing in the code is expected to
change.

## Acceptance

- [ ] `./target/release/examples/dsp_cost --json` completes with **neither** guard firing — no
      `--under-load`, no widened `--load-ceiling` — and the recorded JSON carries the load average
      and the machine it was taken on.
- [ ] The run is recorded under `docs/measurements/` and `docs/measurements/README.md` gains a
      section saying what the figures are, what they are not, and on what machine and date.
- [ ] `docs/specs/call-dsp-effects.md` §10.4 and `docs/specs/call-dsp-noise-reduction.md` §8.1 stop
      saying the figure is untaken and cite the recording instead.
- [ ] The figures are shown able to move: the same harness run against one deliberately heavier
      configuration reports a proportionally heavier cost, and the run says by how much.
- [ ] The full gate is green.

## Progress

- Backlog. Filed by `X-109`, which built the harness and left the figure untaken on purpose.
