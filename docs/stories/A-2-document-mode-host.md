---
id: A-2
title: Implement the document-mode host over the contract interpreter
pillar: Application
status: in-progress
priority:
design: docs/designs/app-host.md
epic: app-host
areas: [sipx-app]
note: app-host phase 1 · needs C-3, C-4, C-5 and M-17 first
---

# Implement the document-mode host over the contract interpreter

## Goal
The first running host in `crates/sipx-app`: answer calls, drive the `sipx-app-protocol`
interpreter (`C-5`), deliver envelopes to a webhook app per
[specs/webhook-binding.md](../specs/webhook-binding.md), execute returned programs.

## Acceptance
- [x] The webhook binding spec's open points are closed and its vectors pass.
- [x] The phase-1 shell proof passes: scripted webhook app + sipx CLI far end → answer,
      prompt, gather, asserted outcome; app stopped → declared `on_unreachable` outcome.
- [x] Declared failure semantics are exercised for timeout, 5xx-past-budget and 4xx — under
      the harness (`A-7`) and once for real.
- [ ] No interpretation of instructions happens outside the `sipx-app-protocol` interpreter
      (review-level check named in the design).
- [x] `sipx-app` stays a leaf: no kernel crate gains a dependency on it, and its own
      dependencies (HTTP, serialization) appear in no other crate's tree.

## Progress
- The webhook binding is closed by WB-1…WB-9: three attempts at fixed 100/200 ms pacing inside
  one callback budget, no redirects, one host-wide `WebhookClient` pool shared by every app,
  bounded response bodies, exact-byte
  redelivery, and one stable `Sipx-Signature` carrying active and retiring key values.
- `sipx-app::host::DocumentCall` is the per-call actor from the design. It owns the call, event
  stream, timers and `sipx_app_protocol::Interpreter`; webhook response bytes enter only as
  `Response::Body`, and the host matches only interpreter outputs. The design names the source
  review that holds this boundary.
- The production document-mode path meets that boundary, but the public `A-7` harness still owns
  and interprets its pre-`C-5` `Instruction`/`Verb` program. Migrating those scenarios to
  `sipx_app_protocol::Interpreter` (or removing the duplicate program) remains required before the
  interpretation checkbox can close and this story can be done.
- The real loopback vectors exercise timeout, exhausted 5xx retries and immediate 4xx, and host
  integration tests assert that each becomes its separately declared SIP refusal. The A-7 shared
  scenarios continue to cover every failure/action pairing on virtual time.
- `crates/sipx-app/tests/document_mode.sh` starts a scripted webhook and the real host, drives it
  from `sipx dial`, asserts prompt audio and the gathered digit, stops the app, and asserts the
  declared 503 unreachable outcome. Readiness is communicated only after each socket is bound;
  every background process has a process-group cleanup trap.
- `sipx-app` is the sole manifest selecting the HTTP client. It remains a leaf, and its first real
  use of the protocol crate graduates the Rust vocabulary, codec, interpreter and call adapter to
  Supported while leaving the wire name under its separate two-application criterion.

## Notes
- `M-18` (mute) and `C-6` (bridge) are not needed for the phase-1 proof; their verbs surface
  as contract errors until those land, which the harness should assert rather than hide.
