---
id: A-4
title: Implement the session binding and originate
pillar: Application
status: backlog
priority:
design: docs/designs/app-host.md
epic: app-host
areas: [sipx-app]
note: app-host phase 2 · WebSocket first; the subprocess variant is this story's decision
---

# Implement the session binding and originate

## Goal
Full-duplex app connections per [specs/session-binding.md](../specs/session-binding.md):
establishment and pinning, liveness, declared backpressure, multiplexing, `originate`.

## Acceptance
- [ ] The session binding spec's open points are closed and its vectors pass — pinning,
      dead-session fan-out to per-call declared semantics, overflow close 1013,
      unknown-call race, originate.
- [ ] A session and a webhook app coexist on one host, and a call's binding is total from
      configuration.
- [ ] The subprocess variant is decided (ship now or defer) and the decision recorded in the
      spec.

## Progress
- Not started.

## Notes
- Shares the failure-semantics engine with `A-2` by construction — one code path, per the
  host design.
