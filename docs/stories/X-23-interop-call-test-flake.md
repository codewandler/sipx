---
id: X-23
title: Find out why an interop call test times out one run in five
pillar: Build
status: ready
priority: 3
design:
epic:
areas: [interop, sipx-cli]
note: found by T-23 — a flake that a suite exists to be believed cannot afford
---

# Find out why an interop call test times out one run in five

## Goal
Make the interop call tests deterministic, or say precisely what makes them not. A suite whose
whole value is "an implementation sipx did not write answered the call" is worth nothing the
moment a red run is something people re-run rather than read.

## Acceptance
- [ ] The failure is reproduced deliberately rather than waited for — a loop that runs the call
      tests against the second peer enough times to make the rate a number, recorded here.
- [ ] The cause is named at `path:line`, on whichever side it turns out to be. It may be sipx, the
      peer's start-up, the harness's readiness marker, or the twenty-second timeout being too
      close to the peer's real answer time; the story is not "make it green", it is "say what it
      was".
- [ ] The fix matches the cause. If it is a readiness gap, the marker waits for the thing that was
      not ready; if the timeout is genuinely too tight, the new bound is justified against a
      measured distribution rather than doubled until quiet.
- [ ] Failing-first test: whatever pins the cause once it is known. If the cause is a race that no
      test can hold still, the story says so explicitly and the bound it settles on is defended
      here instead.

## Progress
- Not started.

## Notes
- Observed during `T-23`'s verification: five runs of `./tests/interop/run.sh --peer asterisk`,
  one of which failed both `an_independent_user_agent_places_a_call_sipx_answers` and its sibling
  on a twenty-second timeout. The other four passed.
- **It is not `T-23`'s diff.** No `sipx-cli` or `sipx-call` code changed in that story, and the
  failing run used a byte-identical test binary to the passing ones. Filed as pre-existing.
- The suspicion worth checking first is peer readiness rather than sipx: `PEER_READY_MARKER` waits
  for a line the peer logs, and a peer that has logged it is not necessarily one whose PJSIP
  endpoint will originate. That would explain why the failure clusters at the start of a run.
- Both call tests failing together in the same run, rather than one, points at something shared —
  which is more consistent with the peer or the harness than with a race inside a single test.
