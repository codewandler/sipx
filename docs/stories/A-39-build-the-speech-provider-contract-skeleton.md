---
id: A-39
title: Build the speech provider contract skeleton
pillar: Application
status: ready
priority: 5
design: docs/designs/local-speech.md
epic: local-speech
areas: [app-sdk, speech, m16]
predicate:
announcement:
note: A-25 specified it and nothing implements it · the registry, session types and selection order every later speech story needs
---

# Build the speech provider contract skeleton

## Goal

Turn `A-25`'s specification into compiling, testable types: the provider registry, the recognition
and synthesis session contracts, the discovery descriptor and the endpoint-default versus per-call
selection order — with no speech provider behind any of it. This is the thing `A-28`, `M-55` and
`M-56` were all written against and that does not exist.

## Acceptance

- [ ] The recognition and synthesis session contracts from the specification exist as public types,
      with the lifecycle events kept disjoint from SIP failure types exactly as the spec requires.
- [ ] A provider registry accepts registrations and resolves a provider by identity. The discovery
      descriptor carries identity, offline status, languages, voices, formats and devices.
- [ ] Endpoint-default and per-call override precedence is implemented with the spec's typed refusal
      order. A failing-first test proves each refusal is distinguishable, and that an unknown or
      unavailable provider is refused before any call resource is taken.
- [ ] The specification's conformance vectors run against a deliberately inert in-repo test provider
      that implements the contract and recognizes nothing — proving the contract is executable
      without implying a capability.
- [ ] No speech recognition or synthesis implementation, model, accelerator dependency or audio
      retention ships in this story, and the public documentation does not present speech as an
      available capability.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from the rc.4 readiness audit. `A-25` shipped a specification and nothing else:
  there is no provider registry, session type or speech logging anywhere under `crates/*/src`. That
  left `A-28` unimplementable — three of its acceptance rows have nothing to run against — and the
  same gap sits under `M-55` and `M-56`. This story is the missing predecessor.

## Notes

- Normative contract: `A-25`'s specification under `docs/specs/`. Do not restate or reinterpret it
  here; if the spec is ambiguous, fix the spec.
- `M-54` owns the bounded PCM attachment seam. This story must consume that seam rather than open a
  second tap into call media.
- Ordering: `A-39` → `A-28` (isolation and retention policy) → `M-55`/`M-56` (actual providers) →
  `A-26`/`A-27` (SDK lifecycle).
