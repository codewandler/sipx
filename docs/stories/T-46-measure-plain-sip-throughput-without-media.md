---
id: T-46
title: Measure plain SIP throughput without media
pillar: Transport
status: ready
priority: 12
design:
epic: diagnostic-automation
areas: [sipx-call, sipx-cli]
predicate:
announcement:
note: audio doubles memory and costs 8x on burst setup at 1000 calls · neither example can isolate signalling from the media stack
---

# Measure plain SIP throughput without media

## Goal

Give the load and capacity examples a genuinely media-free mode, so the cost of signalling can be
measured apart from the cost of having a media stack at all.

## Why

Measured, not assumed. At 1000 concurrent calls, `capacity_test` records 231 MB with audio flowing
against 120 MB with a negotiated-but-silent session, and a burst setup median of 300 ms against
39 ms. So media dominates — but the comparison stops short of the question a deployment actually
asks, because **`--media idle` is not "no media"**: the SDP is still negotiated, the RTP socket is
still bound, and the jitter buffer still exists. What `idle` isolates is the cost of *carrying*
audio, not the cost of the stack that carries it.

That matters because a real deployment may put media somewhere else entirely and run sipx as
signalling only. There is no number for that shape today from the examples.

The CLI can already do it — `sipx load --mode signalling` builds INVITE/ACK/BYE with no SDP body and
no media session, and `sipx load-responder` answers the same way. So the capability exists and the
library-facing examples cannot reach it: `dial` always negotiates media, and `Invitation::answer`
always creates a session.

## Acceptance

- [ ] `capacity_test` and `load_test` gain a mode that negotiates no media: no SDP offer, no RTP
      socket bound, on **both** ends. Reusing the CLI's path or lifting it into the library are both
      acceptable; duplicating it in an example is not.
- [ ] A run in that mode records memory and setup latency beside the existing two, so the three
      shapes — no media, idle media, flowing media — are one comparison rather than three runs a
      reader has to align by hand.
- [ ] The `audio_observed` guard keeps holding: a media-free run must report no audio figures at
      all, not zeros.
- [ ] `docs/measurements/README.md` gains the third column and says which of the three a deployment
      running external media should read.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-09: filed. The `capacity_test` doc comment already pointed at this story before it
  existed — a reference to a filing that had not happened, which is the same defect class as a
  ticked acceptance row nobody satisfied. The pointer is now true.
