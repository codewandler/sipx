---
id: X-105
title: Prove speech-provider substitution with one conformance suite
pillar: Build
status: backlog
priority: 27
design: docs/designs/local-speech.md
epic: local-speech
areas: [testkit, app-sdk, speech, conformance, m16]
predicate:
announcement:
note: after M-55, M-56, A-26, A-27 and A-28 · same suite for bundled and downstream providers
---

# Prove speech-provider substitution with one conformance suite

## Goal

Make provider interchangeability an executable contract rather than an interface claim, using the
same test vectors for deterministic doubles, bundled local providers and downstream replacements.

## Acceptance

- [ ] Public testkit suites exercise discovery, format negotiation, partial/final ordering,
      synthesis chunk ordering, cancellation, backpressure, discontinuity and terminal failure.
- [ ] A deterministic recognition and synthesis test provider passes without accelerator, model or
      network access and can inject every lifecycle and failure transition.
- [ ] Both bundled local/offline providers pass the same behavioral suite; provider-specific tests
      may add coverage but cannot replace common assertions.
- [ ] Contract tests prove endpoint-default and per-call selection, unsupported capability refusal,
      explicit fallback and no cross-call state or event leakage.
- [ ] A minimal external-provider fixture compiles and passes using only public traits and types,
      with no dependency on private bundled-provider modules.
- [ ] Testkit docs show downstream authors how to run the suite and the full gate is green.

## Progress

- Backlog. M16 provider-contract exit proof.
