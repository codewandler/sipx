---
id: A-4
title: Implement the session binding and originate
pillar: Application
status: done
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
- [x] The session binding spec's open points are closed and its vectors pass — pinning,
      dead-session fan-out to per-call declared semantics, overflow close 1013,
      unknown-call race, originate.
- [x] A session and a webhook app coexist on one host, and a call's binding is total from
      configuration.
- [x] The subprocess variant is decided (ship now or defer) and the decision recorded in the
      spec.

## Progress

Done. `sipx-app-protocol` owns the bounded correlated session request/reply frames, and `sipx-app`
owns an authenticated WebSocket driver over a registry with the spec's per-app, per-session,
outbound-frame, per-call-document and listener-task bounds. Calls pin least-loaded/oldest and never
migrate; overflow closes 1013 and atomically fans `on_unreachable` into each pinned call's existing
interpreter, while the ordinary call-end/document race returns typed `unknown_call`.

The host binds the configured session listener beside SIP, serves session and webhook apps from one
running configuration, routes inbound calls through a total binding decision, and admits granted
`originate` requests through the real call framework. A successful originate returns only after the
call is owned and pinned to its requesting session. Host shutdown cooperatively stops and joins the
bounded WebSocket, originate and call tasks.

`SB-1` through `SB-7` exercise pinning, per-call death semantics, overflow, the unknown-call race,
real origination, coexistence and total binding. Companion socket vectors cover bearer
authentication, binary-frame close 1003 and correlated frame routing. The spec records the
subprocess binding as deferred until process framing, supervision, restart and cancellation rules
are specified.

## Notes
- Shares the failure-semantics engine with `A-2` by construction — one code path, per the
  host design.
