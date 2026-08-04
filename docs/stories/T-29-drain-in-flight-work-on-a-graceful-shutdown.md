---
id: T-29
title: Drain in-flight work on a graceful shutdown
pillar: Transport
status: backlog
priority: 16
design: docs/designs/demand.md
epic: demand
areas: [sipx-transport, sipx-ua, sipx-call]
predicate:
announcement:
note: restart without dropping calls · labelled critical and never delivered by the surveyed stack
---

# Drain in-flight work on a graceful shutdown

## Goal

Let an endpoint stop accepting new work while finishing what it already has, so a deploy or restart
does not drop established calls and in-flight transactions.

## Acceptance

- [ ] A public drain operation stops accepting new dialogs and new inbound requests that would start
      one, while existing transactions and dialogs continue.
- [ ] Draining is observable: a caller can await completion, and the endpoint reports how much work
      remains. A test asserts the awaited completion happens **after** the last transaction settles,
      driven by the transaction reaching its terminal state rather than by a wall-clock wait.
- [ ] A bounded deadline is supported; on expiry the remaining work is terminated explicitly and the
      termination is counted and logged, never silent.
- [ ] New in-dialog requests on an existing dialog are still served during the drain — a drain that
      breaks live calls is the failure it exists to prevent.
- [ ] Behaviour is stated for each transport, including what happens to pooled connections and to a
      QUIC connection mid-stream.
- [ ] Drain composes with the existing `CancellationToken` and `TaskTracker` shutdown path rather
      than adding a parallel mechanism.
- [ ] Reachable from the CLI so it is testable from a shell, per vision principle 6.
- [ ] `./scripts/gate.py` green, including `check-fixed-sleep.py`.

## Progress
- (not started)

## Notes
- Requested against a comparable stack, labelled critical by its maintainer, still undelivered. It
  is pure user-agent lifecycle and sits squarely in sipx's scope.
- Distinct from overload shedding (`crates/sipx-transport/src/overload.rs`): that refuses new work
  under pressure, this refuses new work under intent. They should share the refusal path where the
  response is the same.
- The related asks about replicating dialog state across instances are **out of scope** — that is the
  clustering platform's concern, and at most sipx exposes serializable state as a hook later.
