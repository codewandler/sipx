---
id: M-61
title: Harden call-audio analysis against adversarial input
pillar: Media
status: backlog
priority: 26
design: docs/designs/call-audio-analysis.md
epic: call-audio-analysis
areas: [sipx-audio, audio-analysis, security, fuzzing, m16]
predicate:
announcement:
note: after M-57 · hostile audio, bounded resources, cross-call isolation and no retention
---

# Harden call-audio analysis against adversarial input

## Goal

Prove that untrusted call audio and hostile lifecycle sequences cannot panic, consume unbounded
resources, starve the media path or leak observations between calls.

## Acceptance

- [ ] Fuzz/property targets cover every supported PCM format with extreme amplitudes, impulses, DC,
      alternating samples, arbitrary chunking, sequence gaps and format changes.
- [ ] Memory and work remain within the M-57 bounds for long silence, permanent activity and event
      consumer backpressure; no network input reaches panic or unsafe code.
- [ ] Cancellation races with frame delivery, reset and event backpressure leave no processor state,
      task or retained raw audio after the call's completion observation.
- [ ] A concurrent-call test uses distinct sentinel streams and proves no state, metric or event can
      cross call boundaries.
- [ ] Ordinary diagnostics contain algorithm/profile identity and counters but no raw audio, and any
      explicit capture facility remains outside this epic.
- [ ] The adversarial corpus joins the bounded fuzz campaign and the full gate is green.

## Progress

- Backlog. May run after M-57 while M-58 through M-60 are implemented.
