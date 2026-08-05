---
id: A-32
title: Allowlist Realtime requests into phone actions
pillar: Application
status: backlog
priority: 34
design: docs/designs/openai-realtime-phone.md
epic: openai-realtime-phone
areas: [testkit, app-sdk, call-control, tools, openai-realtime, security, m17]
predicate:
announcement:
note: after A-30 and A-33 · model never receives Handle · only capabilities this phone exposes
---

# Allowlist Realtime requests into phone actions

## Goal

Translate correlated function-call items into application-owned requests for a strict subset of the
current phone's capabilities, never into an unrestricted call handle or arbitrary command.

## Acceptance

- [ ] The session tool schema is generated from the phone's current exposed capabilities and policy;
      generated speech, DTMF, mute/unmute, hold/resume, hangup and transfer are initial examples,
      not a hard-coded ceiling.
- [ ] Every public test-phone action is classified in one exhaustive registry as model-exposable or
      forbidden; new actions default to forbidden until they define a closed schema, policy and
      typed outcome mapping, and a completeness test fails when an action is unclassified.
- [ ] The model receives no `Handle`, raw SIP request primitive, arbitrary tool name, shell/network
      executor, credential, address book or route-selection authority.
- [ ] Every function item emits requested, accepted or refused, and completed or failed SDK events
      with call/session/call-ID correlation before its result is returned as a correlated output.
- [ ] A failing-first test proves an unadvertised, malformed or stale tool call cannot mutate the
      phone, while its typed refusal is returned to the correct model call ID.
- [ ] Accepted actions reuse supported SDK operations and preserve each operation's typed terminal
      outcome; completion is never inferred merely from dispatch.
- [ ] Capability changes regenerate or narrow the allowlist atomically, in-flight policy remains
      deterministic and the full gate is green.

## Progress

- Backlog. Depends on A-30 and A-33.
