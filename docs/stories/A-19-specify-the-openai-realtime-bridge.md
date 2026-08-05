---
id: A-19
title: Specify the OpenAI realtime bridge
pillar: Application
status: ready
priority: 1
design: docs/designs/openai.md
epic: openai
areas: [sipx-app, specs]
predicate:
announcement:
note: spec before code — every later story in the epic derives its tests from this document's vectors
---

# Specify the OpenAI realtime bridge

## Goal

Write `docs/specs/openai-realtime.md`: the normative contract for bridging one sipx call leg
to one OpenAI realtime session over a WebSocket, precise enough that the stand-in peer
(A-21) and the bridge (A-22) can each be held to its vectors independently.

## Acceptance

- [ ] `docs/specs/openai-realtime.md` exists and states its stance: normative for this
      workspace, observational toward the vendor, with the date the vendor's published
      documentation was observed recorded in the spec.
- [ ] Endpoint and authentication are pinned: the `wss` URL and model selection, bearer
      authentication carried on the upgrade request, and the credential resolved from a
      *named* secret per `docs/specs/host-config.md` N7 — the spec shows the name form, and
      no secret value ever appears in configuration, logs or errors.
- [ ] Audio is pinned to passthrough: the session is configured for G.711 μ-law or A-law in
      both directions to match the call's negotiated wire format, frames travel as base64
      payload inside events, and the spec states the packet-duration and framing rules with
      byte-level vectors (at least: one append event from a known 20 ms μ-law frame, one
      delta event decoded back to known bytes).
- [ ] The event subset is exhaustive for the bridge: every client event the bridge may send
      and every server event it consumes is named with a JSON vector; an event outside the
      subset has a defined disposition (ignored with a counter, or session-fatal) — nothing
      is left "whatever the implementation does".
- [ ] The barge-in rule is normative: on the server's speech-started event the bridge cancels
      the in-flight response and drops its locally queued agent audio; the spec states the
      queue-depth bound so A-22's test asserts a number.
- [ ] Buffering and backpressure are normative: every queue in both directions is bounded
      with its size named, loss is counted per the session-binding discipline, and no rule
      requires a fixed wall-clock wait to stand in for a happens-before.
- [ ] Connection lifecycle and failure taxonomy: socket close or error ends the bridge with
      a typed outcome (no silent reconnect); auth refusal, malformed event, oversize frame
      and stalled peer each have a named outcome and a bound.
- [ ] The vectors are machine-consumable (the way `webhook-binding.md` WB-1…WB-9 are), and
      the spec names which later story owns each vector's enforcement.

## Progress

- 2026-08-05: wrote `docs/specs/openai-realtime.md` on `impl/A-19`. Vendor facts verified
  against OpenAI's published documentation that day (platform guides + API reference at
  `developers.openai.com/api/docs`, and the vendor's published event schemas in
  `openai-python` `src/openai/types/realtime` @ `main`); observation date recorded in the
  spec's §1. Confirmed GA surface: `wss://api.openai.com/v1/realtime?model=…`, bearer on the
  upgrade, no `OpenAI-Beta` header, `audio/pcmu`/`audio/pcma` session formats,
  `response.output_audio.delta`/`.done` (GA names, not the beta `response.audio.*`).
  Vectors ORB-1…ORB-17 with owners A-20/A-21/A-22/A-23; byte-level base64 literals for two
  160-byte G.711 frames. Two deliberate calls recorded in the spec: `interrupt_response:
  false` so cancellation has one owner (the bridge), and `conversation.item.truncate` named
  as a non-goal (vendor recommends it after interruption; the design's client subset omits
  it — §4.3 records the consequence).
- 2026-08-05: review rework (new commit, same branch). Split the barge-in counters —
  `bridge_barge_in_flushed` counts queue *frames* and is the ≤ 2048 bound A-22 asserts,
  `bridge_cancelled_deltas` counts post-cancel *events* and carries no bound — resolving
  the §4.3-vs-ORB-8 contradiction. Defined the read-set failure disposition for known
  events (bad/missing `delta`, missing `response_id` → `MalformedEvent`) with new vector
  ORB-18. Pinned both `SetupTimeout` start points (from the 101; from sending
  `session.update`). Stated the uplink-audio fate before `session.updated` (buffered in
  the 32-frame queue) and the accumulator residue's fate at a barge-in flush (discarded,
  uncounted; padding is only for `response.output_audio.done`). §5.4's full-queue policy
  is now stated as this spec's own rather than attributed to session-binding §3.

## Notes

- Design: `docs/designs/openai.md`. Verify event names and session fields against the
  vendor's published Realtime API documentation at writing time — the design doc's sketch is
  a scoping aid, not a source of truth.
- Precedent for shape: `docs/specs/webhook-binding.md` (vectors, failure knobs),
  `docs/specs/session-binding.md` (bounded queues, counted loss).
