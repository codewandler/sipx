---
id: X-23
title: Find out why an interop call test times out one run in five
pillar: Build
status: in-progress
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
- [x] The failure is reproduced deliberately rather than waited for — a loop that runs the call
      tests against the second peer enough times to make the rate a number, recorded here.
- [x] The cause is named at `path:line`, on whichever side it turns out to be. It may be sipx, the
      peer's start-up, the harness's readiness marker, or the twenty-second timeout being too
      close to the peer's real answer time; the story is not "make it green", it is "say what it
      was".
- [x] The fix matches the cause. If it is a readiness gap, the marker waits for the thing that was
      not ready; if the timeout is genuinely too tight, the new bound is justified against a
      measured distribution rather than doubled until quiet.
- [x] Failing-first test: whatever pins the cause once it is known. If the cause is a race that no
      test can hold still, the story says so explicitly and the bound it settles on is defended
      here instead.

## Progress

### The cause

`tests/interop/run.sh` had no mutual exclusion, and everything a run reserves is machine-global:

- `tests/interop/run.sh:135` — `docker rm -f "$PEER_CONTAINER"`, a fixed name, removed
  unconditionally at the start of every run, without asking whether another run is using it.
- `tests/interop/run.sh:88-94` — `cleanup()` removes *every* container carrying the
  `sipx-interop=1` label, not the ones this run created.
- The peer runs `--network host` on fixed ports, and `crates/sipx-cli/tests/interop_call.rs:59`
  binds a fixed 5080 because the peer's contact is written before any test starts.

So a second run's **start-up deletes the first run's peer mid-call**. The victim's call tests
then wait on a peer that no longer exists and both fail on their twenty-second timeout —
together, which is what made the report look like something shared. It is shared: it is the
container.

The reported signature, reproduced verbatim by timing `docker rm -f sipx-asterisk` (the literal
command `run.sh:135` issues) into the call-test window — 3 of 3 attempts:

```
test an_independent_user_agent_answers_a_call_sipx_placed ... FAILED
test an_independent_user_agent_places_a_call_sipx_answers ... FAILED
  panicked at crates/sipx-cli/tests/interop_call.rs:261:10:
  the peer places the call within twenty seconds: Elapsed(())
test result: FAILED. 0 passed; 2 failed; finished in 20.69s
```

### The rate

Each pair below is two runs overlapping, each with its own harness and peer directory — two
worktrees on one machine, which is how the flake was met.

| condition | failed |
|---|---|
| one run at a time, after the fix | 0 of 10 |
| two runs overlapping, before the fix | **12 of 16** |
| two runs overlapping, after the fix | 0 of 16 |

"One run in five" is not a property of the suite; it is how often another run happened to
overlap. Run alone, the suite did not fail once in 10 runs — nor in an earlier 25-run sequential
loop.

### What it was not

Both checked first, both eliminated with measurements:

- **The readiness marker.** `PEER_READY_MARKER` was the prime suspect. The peer reaches
  `Asterisk Ready` about a second after the container starts, and `peer_check`'s `pjsip show
  endpoints` succeeds immediately after — the endpoint that originates is loaded before the
  marker is logged, not after. No readiness gap was observed in 45+ starts.
- **The peer dropping a module.** Its start-up log carries a pjproject `Too many modules
  registered!` assertion, which loses one module. Across 20 consecutive container starts it lost
  the same one every time (`REFER Progress`, unused by these tests), so it is a constant of the
  image and not a source of variance.

### The fix

`tests/interop/run.sh:83-108` takes an exclusive `flock` for the life of a run, so a second run
waits its turn instead of destroying the first. The lock is machine-global because what it guards
is. Where `flock` is unavailable the run proceeds and says loudly what is not being guarded —
refusing would make the harness unrunnable there, and silence would restore the collision.

The timeout was **not** touched: 20 s was never the problem, and no distribution justifies moving
it. No retry was added.

`scripts/test-interop-run.py` pins it, wired into `gate.py` and the `gate` CI job. It drives the
real `run.sh` — copied verbatim, so it cannot drift — against a fixture peer with stub `docker`
and `cargo`, and asserts the two runs' events change hands exactly once. Before the fix it
reported `A A A B B B A A B B A A B`.

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
