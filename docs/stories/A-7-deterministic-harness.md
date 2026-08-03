---
id: A-7
title: The deterministic harness — fake time, scripted bindings, scripted calls
pillar: Application
status: done
priority:
design: docs/designs/app-host.md
epic: app-host
areas: [sipx-app]
note: app-host phase 1 · built with the host, not after it · startable against contract vectors alone
---

# The deterministic harness — fake time, scripted bindings, scripted calls

## Goal
The apparatus every behaviour claim about the host rests on: drive its decision logic with
fake time, a scripted app on any binding, and scripted call events — no sockets, no engine,
no transport endpoint — so the slow app, the flapping app and the absent app are ordinary
test cases.

## Acceptance
- [x] A scenario is data: a sequence of call events, binding responses (with delays) and timer
      firings, plus the expected effects and outcomes.
- [x] The contract's vector set ([`app-contract.md`](../specs/app-contract.md) AC-1 … AC-9)
      runs under the harness as scenarios — which is possible before `C-3`/`C-5` land, and is
      this story's point.
- [x] Failure-semantics scenarios exist for every §9.2 knob and are shared by `A-2`/`A-4`
      rather than rewritten per binding.
- [x] A scenario that needs a real socket or real time is structurally impossible to express —
      the sans-IO discipline one layer up, enforced by the harness's own types.

## Progress
- Done, in `crates/sipx-app/src/harness/` — the crate's first code, as the design says it should be.
  Six modules: `time` (the clock), `contract` (the vocabulary), `policy` (§9.2), `binding` (the
  app), `scenario` (the model and its runner), `vectors` (§11 and the knobs). 20 tests.
- **A scenario is data, and so is its expectation.** `Scenario` carries the app's script with
  per-reply delays, the call events in time order, redeliveries, and the failure declaration;
  `Expectation` carries the effects, the conclusion, the delivered `seq`s, the knobs consulted, the
  legs and what must *not* have happened. Keeping the expectation data rather than a closure is
  what lets `A-2`/`A-4` reuse a vector's verdict instead of restating it.
- **AC-1 … AC-9 all run**, before `C-3` and `C-5` existed, which was the story's point. The harness
  still carries the provisional instruction-execution half it needed then. `C-5` has since landed,
  so migrating that half to `sipx_app_protocol::Interpreter` is now an open `A-2` requirement;
  until then these scenarios remain useful actor-policy evidence but do not prove a
  sole-interpreter architecture.
- **Acceptance 4 is enforced by a type, not by review.** `Binding::respond` returns what the app
  *will* say **and how long that will take**. A real HTTP client cannot answer the second half
  before making the call, so it cannot implement the trait in good faith; combined with a `Virtual`
  clock that has no `now()`, a scenario whose outcome depends on the machine running it cannot be
  written down. A test asserts that a minute of virtual time costs under 250 ms of real time.
- Every §9.2 knob has a scenario **per declared action** — twelve, generated from `Failure::all()`
  so adding a fifth knob without a scenario fails a test rather than going quietly untested. A test
  also shows a foreign binding being held to them, and being *failed* when it misbehaves.
- **Two readings the spec leaves implicit** are recorded in `scenario`'s module docs, because a
  vector's outcome depends on each. The protocol interpreter now owns the normative reading; the
  provisional harness interpreter must be migrated rather than treated as a second authority:
  1. An empty document does not replace the program — §6.3 calls a response "the entire new
     program" *and* calls an empty one "keep going", which only reconcile if empty changes nothing.
  2. A digit consumed by a running `gather` is not also delivered as `call.dtmf`. AC-3's barge-in is
     a digit during a `play`, where no gather runs, so that case is untouched.
- AC-1's wording says "after `timeout_ms`", but the harness fails an unreachable app immediately:
  §9.2 gives "absent" and "slow" separate knobs, and a host that could not tell them apart could not
  honour both. Which one a connect failure reports is `A-2`'s call; the vector's real claim — the
  declared effect, nothing else, no hang — is what is checked.
- Two bugs the vectors caught while being written, both worth keeping: a failure fired *again* when
  the app answered the `call.ended` it had itself caused, and a synchronous failure left the queued
  events undelivered because nothing pumped the queue after it.
- Mutation-tested: appending instead of replacing (AC-3), applying a redelivery's answer (AC-4),
  letting `call.ended` be dropped like any other event (AC-9), and skipping an unknown verb instead
  of rejecting the document whole (AC-5) — each fails a vector.
- The harness schedules on `sipx-transport`'s own `TimerQueue`, which `X-21` made generic over its
  instant. This is that parameter's second caller and the reason it was worth adding.

## Notes
- If scenario-driving the interpreter proves generally useful, consider promoting that part
  into `sipx-testkit`; the binding and failure-semantics layers stay in `sipx-app`.
