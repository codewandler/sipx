---
id: C-6
title: Reach the bridge and the conference from a call
pillar: Signalling
status: backlog
priority:
design: docs/designs/app-sdk.md
epic: app-sdk
areas: [sipx-call, sipx-media]
note: app-sdk · last; not v1-blocking · C-1 (M9) later upgrades the signalling half · size M
---

# Reach the bridge and the conference from a call

## Goal
Two calls a host owns can be bridged — and several joined to a conference — through the public
API, without `Arc<Mutex<Call>>` and without constructing media sessions from raw ports.

## Acceptance
- [ ] Two `Call`s can be connected so audio passes between them, using `M-11`'s bridge
      underneath; the connection is made through the calls' public API, and `Call`'s ownership
      story survives it — media sharing stays channel-backed, no shared mutable session
      (the vision's principle 3).
- [ ] DTMF while bridged has a declared, tested behaviour: pass through or deliver to the host,
      selectable when the bridge is made.
- [ ] Unbridging returns both calls to independent operation; ending either call ends the bridge
      and is observable on the other's event stream (`C-3`).
- [ ] A `Call` can join and leave `M-12`'s conference through the same public path.
- [ ] Failing-first test: `two_calls_bridge_and_pass_audio`.

## Progress
- Not started.

## Notes
- Today `Bridge::connect(Arc<MediaSession>, Arc<MediaSession>)` and `Conference::join` exist
  only at the media layer and are unreachable from `Call`, which holds its `MediaSession` by
  value and lends `&MediaSession` (`crates/sipx-media/src/bridge.rs`,
  `crates/sipx-call/src/call.rs`). Only tests construct bridges, from raw ports.
- Scope: the **media** coupling of two host-owned calls. The signalling coupling — offer relay
  on every axis, glare, CANCEL/BYE mapping — is `C-1`, deliberately later (M9, after `S-19` and
  `C-2`), and upgrades the contract's `bridge` verb without changing it.
- Needed by the host (`crates/sipx-app`): `bridge` and `dial`-then-connect are contract
  verbs.
