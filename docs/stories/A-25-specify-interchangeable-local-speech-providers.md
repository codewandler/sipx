---
id: A-25
title: Specify interchangeable local speech providers
pillar: Application
status: backlog
priority: 20
design: docs/designs/local-speech.md
epic: local-speech
areas: [app-sdk, speech, audio, m16]
predicate:
announcement:
note: M16 spec gate · endpoint default and per-call override · local/offline by default
---

# Specify interchangeable local speech providers

## Goal

Define stable, separate recognition and synthesis provider contracts before implementation so an
endpoint or call can select a local implementation or a downstream replacement without changing
application code.

## Acceptance

- [ ] A normative spec defines recognition and synthesis inputs, outputs, ownership, bounded
      queues, cancellation, failure and shutdown without putting I/O or clock reads in a core crate.
- [ ] Discovery reports provider identity, local/offline status, languages, voices, accepted and
      emitted sample formats, streaming support, execution devices and resource estimates.
- [ ] Endpoint defaults and per-call overrides have deterministic precedence; unsupported language,
      voice, format or device selection returns a typed reason and never silently changes policy.
- [ ] The spec defines warm-up, readiness, provider loss, explicit fallback and cancellation events
      and distinguishes them from SIP call failures.
- [ ] Conformance vectors cover a deterministic test provider, replacement ordering, backpressure,
      discontinuity, cancellation and terminal failure for both interfaces.
- [ ] The public API review records which types can be extended compatibly and the full gate is green.

## Progress

- Backlog. M16 admission and contract gate.
