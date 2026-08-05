---
id: M-64
title: Attach bounded DSP graphs to calls
pillar: Media
status: backlog
priority: 38
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-call, sipx-media, dsp, realtime, m18]
predicate:
announcement:
note: after M-54 and M-63 · ordered per-direction graphs, atomic replacement and teardown barrier
---

# Attach bounded DSP graphs to calls

## Goal

Run validated ordered processor chains on transmit and receive PCM without partial graph updates or
one call's DSP delaying another call.

## Acceptance

- [ ] Calls attach separate ordered transmit/receive graphs through M-54; the complete graph is
      validated before an atomic activation or replacement.
- [ ] Queue, frame, scratch, retained tail and processor-count limits are explicit and non-zero;
      configuration cannot create an unbounded chain or delay line.
- [ ] Each frame sees exactly one graph generation, and bypass/removal/replacement has a typed
      sample-boundary transition with no mixture of old and new parameter state.
- [ ] Slow or failed processors follow M-63's minimum configured failure policy without awaiting
      application work or stalling RTP on proven-inline/supervised-isolated profiles, and
      discontinuity reaches every downstream processor.
- [ ] Supervised external workers use bounded request/result channels; deadline, crash or malformed
      result applies the declared action, and cancellation terminates and reaps the worker process.
- [ ] Drop/cancel/call teardown waits on an observable barrier proving zero graph tasks, frames and
      processor state; concurrent calls share no mutable DSP state.
- [ ] Failing-first live-call tests, feature combinations, strict clippy and the full gate are green.

## Progress

- Backlog. Depends on M-54 and M-63; M-68 later hardens and proves this policy.
