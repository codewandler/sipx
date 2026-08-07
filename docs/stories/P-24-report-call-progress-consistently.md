---
id: P-24
title: "Report call progress consistently"
pillar: "Phone"
status: in-progress
epic: diagnostic-automation
areas: [sipx-cli]
design: docs/designs/diagnostic-automation.md
note: "follow-up external review finding 11 · answer and load do not emit the INFO progress promised for -v"
---

# Report call progress consistently

## Goal

Make one verbosity level narrate the same useful call lifecycle in caller, answerer and bounded-load
roles without contaminating result stdout or multiplying per-call output beyond configured bounds.

## Acceptance

- [x] The CLI reference defines the INFO event vocabulary and fields for waiting, placed,
      answered, caller observed, ended and load-summary progress, including which high-volume load
      events are sampled or aggregated.
- [x] Failing-first two-process tests reproduce the review: `dial -v` emits calling/answered/end,
      `answer -v` emits only waiting, and `load -v` emits no progress.
- [x] `answer -v` names the caller after admission and the terminal cause after joined teardown.
      `load -v` reports bounded admission and completion progress without one unbounded log per
      attempted call.
- [x] Lifecycle records are emitted from typed state transitions shared with result construction,
      not inferred from elapsed sleeps or duplicated command-side guesses.
- [x] INFO remains on stderr in text and JSON modes. Default verbosity remains quiet, repeated `v`
      retains its documented saturation, and no result schema changes merely to support logging.
- [x] Remote hangup, refusal, timeout, interruption and internal failure each produce a truthful
      final progress event with no duplicate end when causes race.
- [ ] Focused logging/process tests, README/help/reference wording and the complete repository gate
      are green.

## Review evidence

The follow-up review observed the documented three-event lifecycle from `dial -v`, only a waiting
line from `answer -v`, and no output from `load -v` during a completed run.

## Progress

- `docs/specs/diagnostic-phone.md` section 6.4 defines the stable INFO event vocabulary, typed
  terminal causes, exact dial/answer ordering and bounded aggregate load policy. DPH-23 through
  DPH-25 carry the process vectors. Board regeneration and the complete gate remain deferred to
  push.
- `progress::Call` now owns one role/peer clock and the first typed terminal cause; the same cause
  supplies terminal result words and exactly one INFO end record. Because the owner is declared
  before transport/call resources, its internal-failure fallback runs after their drop. Dial and
  answer use the typed placed/waiting, caller-observed, answered and ended transitions. Load emits
  its start once and its summary from the same aggregate facts as `sipx.load.v1`, with no per-call
  INFO.
- The two failing-first regressions now pass, as do focused text/JSON stderr separation, busy
  refusal, cancellation timeout, waiting and pending interruption, transport failure, internal
  load failure and first-cause tests. All 114 CLI unit tests, default/all-feature strict lints,
  executable help/reference, public-content, documentation-link, fixed-sleep and provenance checks
  pass. README/help/reference wording is synchronized. The complete gate and board regeneration
  remain deferred to push.
