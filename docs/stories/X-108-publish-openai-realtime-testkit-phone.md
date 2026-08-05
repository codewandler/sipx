---
id: X-108
title: Publish the OpenAI Realtime testkit phone and measurements
pillar: Build
status: backlog
priority: 36
design: docs/designs/openai-realtime-phone.md
epic: openai-realtime-phone
areas: [testkit, example, documentation, metrics, openai-realtime, m17]
predicate:
announcement:
note: M17 exit after X-107 · runnable phone, policy UI and bounded cost/rate/latency evidence
---

# Publish the OpenAI Realtime testkit phone and measurements

## Goal

Publish a runnable phone that lets a live far-end caller converse with a configured Realtime model
and request policy-governed phone actions, alongside honest rate, usage, cost and latency evidence.

## Acceptance

- [ ] The example registers or answers through the supported testkit phone, streams two-way audio,
      shows typed understanding and lifecycle events, and demonstrates interruption cleanup.
- [ ] Its UI/config exposes the exact generated allowlist across the supported test-phone action
      registry plus requested/accepted/refused/completed events and any required confirmation; the
      example includes speech and DTMF, and no `Handle` escapes.
- [ ] A bounded live run records input-to-transcript, input-to-first-audio and output-playback
      latency, queue pressure, interruption/truncation, rate-limit events and action outcomes.
- [ ] Usage and cost evidence comes from returned usage/configured accounting with model, date,
      duration and limits; documentation does not hard-code a price or extrapolate beyond the run.
- [ ] Setup uses `OPENAI_API_KEY` only in the server-side process, embeds no credential, redacts live
      artifacts and documents explicit opt-in, budgets, privacy and teardown.
- [ ] A clean consumer runs the deterministic example without a key, the opt-in live path reuses the
      same packaged surface, docs/link checks pass and the full gate is green.

## Progress

- Backlog. Final M17 example and measurement after X-107.
