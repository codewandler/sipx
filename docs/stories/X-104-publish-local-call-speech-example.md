---
id: X-104
title: Publish a runnable local live-call speech example and measurements
pillar: Build
status: backlog
priority: 28
design: docs/designs/local-speech.md
epic: local-speech
areas: [example, app-sdk, speech, gpu, cpu, documentation, m16]
predicate:
announcement:
note: M16 exit after X-105 · accelerator when available and bounded CPU fixture everywhere
---

# Publish a runnable local live-call speech example and measurements

## Goal

Show a packaged application transcribing far-end speech and synthesizing a response into a live call
through only the supported SDK, with honest accelerator and CPU limits.

## Acceptance

- [ ] The runnable example configures endpoint defaults, exercises a per-call override, receives
      partial/final recognition events and plays then cancels synthesized speech on a real call.
- [ ] It uses a bundled local/offline provider, performs no implicit network request, retains no
      audio or text by default and prints no credential, model path or transcript in ordinary logs.
- [ ] An accelerator run records hardware, provider capability, real-time factor, start latency,
      steady-state CPU, accelerator memory and process memory from the packaged example.
- [ ] A bounded CPU-only fixture runs without special hardware in CI and proves the declared CPU
      behavior, lifecycle and cleanup; CI does not pretend this is an accelerator measurement.
- [ ] Documentation covers installation, model assets, explicit fallback, privacy, troubleshooting
      and how to substitute another provider through X-105's contract.
- [ ] The clean-consumer test installs the packaged surface, the public docs build and link checks
      pass, and the full gate is green.

## Progress

- Backlog. Final local-speech example after X-105.
