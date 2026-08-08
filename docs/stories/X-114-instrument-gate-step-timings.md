---
id: X-114
title: Instrument gate step timings
pillar: Build
status: done
priority:
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

- [x] Every gate step records its wall-clock duration, and the run reports them ordered by cost so
      the expensive tail is visible without reading a log.
- [x] Timings are written to a machine-readable file, not only printed. A recorded run states its
      commit, host CPU count and whether the build cache was cold or warm — a duration without
      those is not comparable to another.
- [x] The run reports total wall clock separately from the sum of step durations, so parallelism and
      serialization are distinguishable.
- [x] Nothing gates on a duration. A slow run is never a failed run — the same rule `X-66` follows
      for coverage, and for the same reason.
- [x] A failing-first test proves a step whose duration is missing or unparseable is reported rather
      than silently dropped.
- [x] `./scripts/gate.py` green, and `gate.py --check` still accounts for every CI job. *`--check`
      holds — 40 steps over 21 CI jobs, none unaccounted for, and no CI job was added. The full
      sweep was not run: this wave's dispatch withholds it because it is ~30 minutes per implementor
      and the diff is Python-only. The steps it can reach were run instead (below).*

## Progress

- 2026-08-08: filed from the `rc.4` readiness audit and from this session's own throughput problem.
  The audit found `X-93`'s `12m37`/`6m41`/`13m19` baseline appears in no release record, review or
  changelog — it exists only as prose inside `X-93` itself — and that `gate.py` has no clock at all:
  it prints a step banner and a step count, and its only quantitative instrumentation is free disk.
- 2026-08-08: implemented in `scripts/gate.py` under a new "The clock" section. Every step is timed,
  including the ones that fail, disclaim, or are never reached — an unreached step keeps its row and
  says `not started`, because a table the reader can count has to hold every step. The summary
  prints before the verdict so the verdict stays last, and the record lands in
  `<target>/gate-timings.json`; `--timings PATH` puts it somewhere `cargo clean` will not remove.

  Three things worth carrying into `X-93`:

  * **`release rehearsal tests` is 3m34s** — 58% of the whole non-cargo half of the gate, and about
    six times the next Python step (`maturity tests`, 56s). Nothing about it is a build. It is the
    single cheapest thing to look at first, and nobody knew because nothing measured it.
  * **`cold` is now three-valued.** With `sccache` in `RUSTC_WRAPPER` an empty `target/` no longer
    means every crate was compiled, so `cold target, warm compiler cache` is recorded separately.
    Comparing one of those against `X-93`'s pre-wrapper `12m37` would read a cache hit as a speed-up.
  * **Load is recorded, and it matters here.** The prefix above was measured at load 31 on a 20-CPU
    host, because this project runs several implementor gates at once. A CPU count alone says how
    many cores exist, not how many a run had.

  Deliberately *not* done: no committed timings document. A duration is a fact about one machine at
  one moment, and a committed one would need a staleness rule to stay true — the story rules that
  out in as many words ("instrumentation that itself needs maintenance to stay true is worse than
  none").

- 2026-08-08: closed in the `1.0.0-rc.5` boundary.

## Notes

- Sequence: `X-93` cannot start before this lands. Filing `X-93`'s registry-pacing acceptance row as
  its own story is also outstanding — that row shares nothing with the rest of it.
- The immediate consumer is not only `X-93`. A wave of story implementors currently pays the gate's
  cost repeatedly, and choosing what to stop running requires knowing what each step costs.
- Keep it cheap. Instrumentation that itself needs maintenance to stay true is worse than none; a
  duration and a context record are enough.
