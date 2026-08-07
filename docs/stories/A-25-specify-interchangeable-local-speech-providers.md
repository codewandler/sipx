---
id: A-25
title: Specify interchangeable local speech providers
pillar: Application
status: in-progress
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

- [x] A normative spec defines recognition and synthesis inputs, outputs, ownership, bounded
      queues, cancellation, failure and shutdown without putting I/O or clock reads in a core crate.
- [x] Discovery reports provider identity, local/offline status, languages, voices, accepted and
      emitted sample formats, streaming support, execution devices and resource estimates.
- [x] Endpoint defaults and per-call overrides have deterministic precedence; unsupported language,
      voice, format or device selection returns a typed reason and never silently changes policy.
- [x] The spec defines warm-up, readiness, provider loss, explicit fallback and cancellation events
      and distinguishes them from SIP call failures.
- [x] Conformance vectors cover a deterministic test provider, replacement ordering, backpressure,
      discontinuity, cancellation and terminal failure for both interfaces.
- [ ] The public API review records which types can be extended compatibly and the full gate is green.

## Progress

- Spec landed as [`docs/specs/speech-providers.md`](../specs/speech-providers.md): sans-I/O contract
  placement (§2), discovery descriptor (§3), selection precedence with typed refusal order (§4),
  recognition and synthesis session contracts (§5–§6), lifecycle disjoint from SIP (§7), default
  bounds (§8), the prospective public API review (§9) and 33 conformance vectors (§10) that seed
  X-105's suites. No code was written; M-54/M-55/M-56/A-26/A-27 implement against this contract.
- The §9 extensibility record is written against planned types; the row above also requires the
  full gate, so it is ticked only once the gate run for this change is green. Re-verify §9 against
  the real public API in the first implementation story that introduces the types (M-54 or M-55).
