---
id: A-28
title: Isolate speech data and resources with no default retention
pillar: Application
status: backlog
priority: 22
design: docs/designs/local-speech.md
epic: local-speech
areas: [app-sdk, speech, privacy, security, m16]
predicate:
announcement:
note: after A-25 · gates provider delivery · explicit opt-in for retention or off-host processing
---

# Isolate speech data and resources with no default retention

## Goal

Make local speech processing private and bounded by default, with per-call ownership and explicit
host policy for every operation that retains data or sends it off the machine.

## Acceptance

- [ ] The spec classifies audio, transcript, synthesized text, model state, credentials and derived
      caches, and sets no retention beyond the live operation as the default for user data.
- [ ] Debug capture, persistent derived data and an off-host provider each require explicit host
      configuration and are visible through provider discovery and call events.
- [ ] Every call has independent bounded queues, execution budget, cancellation and provider state;
      a failing-first concurrency test proves one call cannot receive another call's data or events.
- [ ] Ordinary logs redact credentials, model paths, transcript text and synthesis input, while
      still reporting typed provider identity, lifecycle, limits and failure causes.
- [ ] Cancellation and provider failure erase transient buffers and release accelerator and CPU
      resources; tests inspect cleanup instead of relying on elapsed time.
- [ ] The public privacy guide states local/offline defaults, opt-ins and operational limits, and the
      full gate is green.

## Progress

- Backlog. Follows A-25 and precedes the shipped providers.
