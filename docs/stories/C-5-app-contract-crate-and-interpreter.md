---
id: C-5
title: The application contract crate and its sans-IO interpreter
pillar: Application
status: ready
priority: 2
design: docs/designs/app-sdk.md
epic: app-sdk
areas: [sipx-app-protocol]
note: app-sdk · parallel to C-3/C-4/M-17/M-18 · spec is docs/specs/app-contract.md · size M
---

# The application contract crate and its sans-IO interpreter

## Goal
A new crate — working name `sipx-app-protocol` — that owns the `sipx.app.v1` types and an
instruction interpreter that is a pure state machine: call events and an instruction program in,
effects out. The primitive every host is a driver over.

## Acceptance
- [ ] The crate defines the event, instruction and envelope types of
      [`docs/specs/app-contract.md`](../specs/app-contract.md), with serialization confined to
      this crate — `sipx-call` and the rest of the workspace gain no new dependencies.
- [ ] The interpreter consumes `CallEvent`s (`C-3`) and instruction documents, and yields typed
      effects that map one-to-one onto `sipx-call`/`sipx-media` operations. It contains no
      socket, no clock read, and no async runtime; time enters as fired-timer inputs.
- [ ] The spec's continuation rule is enforced by construction: at most one outstanding
      callback per call, and a document accepted in response to event E **replaces** the pending
      instruction queue. The spec's vectors for this rule pass.
- [ ] Every vector table in the spec has a derived test, and the interpreter passes all of them.
- [ ] An `examples/` binary drives the interpreter over a real call with a canned program
      (answer → play → gather → hang up), and a shell script asserts the outcome — the epic's
      end-to-end proof, with no host and no workspace-wide dependency additions.
- [ ] The crate is marked experimental in its README and crate docs, matching the spec's status.
- [ ] Failing-first test: the spec's vector `AC-1` (event in, no program loaded → the defined
      default effect, not a panic).

## Progress
- Not started. Blocked on the spec landing (same epic) and on `C-3` for the event input type.

## Notes
- The interpreter is the primitive every binding drives — remote or in-process — which is why
  it lives in its own crate and not inside `sipx-app`. See the design's "Alternatives
  considered" for the placements weighed.
- The host (`crates/sipx-app`, the [app-host](../designs/app-host.md) epic) implements the
  webhook, socket and embedded-runtime bindings of this same vocabulary; `A-2` needs this story.
