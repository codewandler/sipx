---
id: A-7
title: The deterministic harness — fake time, scripted bindings, scripted calls
pillar: Application
status: ready
priority: 6
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
- [ ] A scenario is data: a sequence of call events, binding responses (with delays) and timer
      firings, plus the expected effects and outcomes.
- [ ] The contract's vector set ([`app-contract.md`](../specs/app-contract.md) AC-1 … AC-9)
      runs under the harness as scenarios — which is possible before `C-3`/`C-5` land, and is
      this story's point.
- [ ] Failure-semantics scenarios exist for every §9.2 knob and are shared by `A-2`/`A-4`
      rather than rewritten per binding.
- [ ] A scenario that needs a real socket or real time is structurally impossible to express —
      the sans-IO discipline one layer up, enforced by the harness's own types.

## Progress
- Not started. Startable immediately: depends only on the contract spec.

## Notes
- If scenario-driving the interpreter proves generally useful, consider promoting that part
  into `sipx-testkit`; the binding and failure-semantics layers stay in `sipx-app`.
