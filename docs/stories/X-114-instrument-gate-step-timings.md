---
id: X-114
title: Instrument gate step timings
pillar: Build
status: ready
priority: 16
design:
epic: conformance
areas: [scripts, ci]
predicate:
announcement:
note: X-93's baseline exists only as prose in its own story · gate.py has no clock at all, so nothing can be shown to have got faster
---

# Instrument gate step timings

## Goal

Give `./scripts/gate.py` a clock, so "the gate got faster" is a measurement rather than a
recollection. This is `X-93`'s missing prerequisite: that story asks for protected release evidence
to be faster without weakening it, and today there is nothing to compare against.

## Acceptance

- [ ] Every gate step records its wall-clock duration, and the run reports them ordered by cost so
      the expensive tail is visible without reading a log.
- [ ] Timings are written to a machine-readable file, not only printed. A recorded run states its
      commit, host CPU count and whether the build cache was cold or warm — a duration without
      those is not comparable to another.
- [ ] The run reports total wall clock separately from the sum of step durations, so parallelism and
      serialization are distinguishable.
- [ ] Nothing gates on a duration. A slow run is never a failed run — the same rule `X-66` follows
      for coverage, and for the same reason.
- [ ] A failing-first test proves a step whose duration is missing or unparseable is reported rather
      than silently dropped.
- [ ] `./scripts/gate.py` green, and `gate.py --check` still accounts for every CI job.

## Progress

- 2026-08-08: filed from the `rc.4` readiness audit and from this session's own throughput problem.
  The audit found `X-93`'s `12m37`/`6m41`/`13m19` baseline appears in no release record, review or
  changelog — it exists only as prose inside `X-93` itself — and that `gate.py` has no clock at all:
  it prints a step banner and a step count, and its only quantitative instrumentation is free disk.

## Notes

- Sequence: `X-93` cannot start before this lands. Filing `X-93`'s registry-pacing acceptance row as
  its own story is also outstanding — that row shares nothing with the rest of it.
- The immediate consumer is not only `X-93`. A wave of story implementors currently pays the gate's
  cost repeatedly, and choosing what to stop running requires knowing what each step costs.
- Keep it cheap. Instrumentation that itself needs maintenance to stay true is worse than none; a
  duration and a context record are enough.
