---
id: A-2
title: Implement the document-mode host over the contract interpreter
pillar: Application
status: done
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
- [x] No interpretation of instructions happens outside the `sipx-app-protocol` interpreter
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
- The public `A-7` harness is now a virtual-time driver over the same
  `sipx_app_protocol::Interpreter`: its contract module re-exports the protocol vocabulary, while
  its scenario runner performs `Output`, feeds `Input`, and owns no instruction queue or verb
  dispatch. A source-level regression refuses `Document::parse`, `Verb::`, or instruction-field
  access in the runner. The migration moved the last old guard it exposed — a failed or timed-out
  answer to `call.ended` could issue a second teardown — into the sole interpreter for every
  binding.
- The real loopback vectors exercise timeout, exhausted 5xx retries and immediate 4xx, and host
  integration tests assert that each becomes its separately declared SIP refusal. The A-7 shared
  scenarios continue to cover every failure/action pairing on virtual time.
- An outstanding HTTP callback does not stop the call actor: it continues polling call events,
  routed in-dialog requests, digits and timers. A live regression holds a webhook callback past its
  declared timeout while the peer sends BYE, then requires the BYE's 200 and a final
  `call.ended` callback. The interpreter treats an ended snapshot as authoritative even while that
  terminal event waits behind an earlier callback, suppressing a second teardown while still
  draining the terminal envelope.
- `Host::serve` owns a bounded actor supervisor. Ordinary shutdown joins it; cancellation signals
  cooperative refusal or BYE and leaves the supervisor owning teardown until every actor exits.
  Actor-owned, generation-scoped admission leases release `Running` even after the serving future
  is gone without allowing an old completion to retire a reused Call-ID. The 1024-actor ceiling
  returns 503 deterministically when saturated.
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
