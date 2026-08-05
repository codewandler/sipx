---
id: A-22
title: Bridge a call to an OpenAI agent
pillar: Application
status: backlog
priority: 3
design: docs/designs/openai.md
epic: openai
areas: [sipx-app, sipx-cli, sipx-media]
predicate:
announcement:
note: integrates A-19–A-21 — the bridge component, its host configuration, and the one-command product path
---

# Bridge a call to an OpenAI agent

## Goal

An application component in `sipx-app` that holds one call leg and one realtime session
together under the host's discipline, plus the configuration and CLI path that make "dial
in, an agent answers" demonstrable with one command — proven end to end against A-21's
stand-in peer.

## Acceptance

- [ ] The bridge owns one `Call` and one realtime session: caller audio leaves
      `recv_encoded` (relay mode) and arrives at the peer as the spec's append events;
      agent deltas queue in a bounded local buffer and leave through `send_encoded`; both
      queues carry the spec's named sizes and counted drops. G.711 passthrough — no
      transcode on the path, asserted by byte identity in a test.
- [ ] Barge-in holds to the spec: on speech-started the bridge cancels the response and
      drops its queued agent audio; the test asserts the spec's queue-depth bound as a
      number, against the stand-in's cancel-honouring mode.
- [ ] Configuration follows host-config discipline: endpoint URL, model, instructions and
      the credential's secret *name* under `[app.<name>]`, unknown keys refused (N2),
      deny-by-default grants (N5), secret values never in config, logs or errors (N7) —
      with config vectors in the existing `HC-*` style for the new table.
- [ ] A CLI path answers (or originates) a call and hands it to the bridge, so one command
      against the stand-in peer demonstrates the loop; its JSON output names the negotiated
      facts (codec, packet duration, session outcome).
- [ ] End-to-end against the stand-in, M-39 evidence pattern: a distinct tone up-path
      arrives in the peer's appends; the peer's distinct tone down-path arrives in the
      call's RTP; correlation asserted both directions; non-vacuity negatives that must
      fail — wrong bearer never bridges, a stalled peer ends the bridge within its bound
      with the typed outcome, a malformed event follows the spec's disposition.
- [ ] A dropped socket ends the bridge with its typed outcome and the call is released the
      way the spec says — no silent reconnect, no orphaned media tasks
      (cancellation-safety asserted).
- [ ] Gate green, including `check-app-surface.py` (deliberate surface growth named in
      Progress) and `check-fixed-sleep.py`.

## Progress

- (running log / checklist — a resuming agent reads this to know exactly where things stand)

## Notes

- Design: `docs/designs/openai.md` component 4. Blocked on A-19 (spec), A-20 (WSS client),
  A-21 (the peer its proof runs against).
- `MediaSession::set_relay(true)` + `recv_encoded`/`send_encoded` are the passthrough
  primitives (`crates/sipx-media/src/session.rs`); `Call::media()` exposes them.
- Whether the product path is a new CLI verb or an app-host binding mode is the
  implementor's call within the design's constraint: one command, host discipline, no
  second config language.
