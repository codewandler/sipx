---
title: The experimental contract
description: A tour of sipx.app.v1, whose types and interpreter exist but whose webhook, session, and embedded bindings do not.
---

# The experimental contract

:::caution Experimental and not connected to customer code

The contract types, JSON format, interpreter, and deterministic vectors exist. `sipx-host` answers
calls, but it does not deliver these events to a webhook or session and does not run an embedded
handler. The wire shape may change before those bindings ship.

:::

The normative definition, including validation, ordering, failure semantics, authentication, and
test vectors, is
[`docs/specs/app-contract.md`](https://github.com/codewandler/sipx/blob/main/docs/specs/app-contract.md).
This page is a non-normative tour.

## Event envelope

An event identifies the contract version and sequence and carries a call snapshot:

```json
{
  "contract": "sipx.app.v1",
  "seq": 4,
  "at": "2026-07-28T09:15:04.221Z",
  "call": {
    "id": "b7c1…",
    "direction": "inbound",
    "state": "answered",
    "from": "sip:alice@example.com",
    "to": "sip:support@example.net",
    "media": { "encrypted": true, "on_hold": false, "muted": false }
  },
  "event": { "type": "call.dtmf", "digit": "5", "duration_ms": 160 }
}
```

The vocabulary covers incoming, ringing, answered, and ended calls; DTMF; playback, gather,
recording, and dial completion; transfer progress; bridge state; and hold state.

## Instruction program

Customer code answers with an ordered program. Its instruction identifiers are echoed by the
corresponding completion events:

```json
{
  "contract": "sipx.app.v1",
  "instructions": [
    { "id": "p1", "do": "play", "source": { "file": "welcome.wav" }, "interruptible": true },
    { "id": "g1", "do": "gather", "max": 4, "terminators": "#", "timeout_ms": 10000 }
  ]
}
```

The vocabulary includes answer, ring, reject, play, gather, record, DTMF, dial, bridge, hold,
mute, transfer, pause, tag, and hangup operations. A word in the contract is not itself evidence
that the current host or public call API can perform that operation end to end.

## Replacement and ordering

In webhook mode, a response replaces the entire pending program. Responding to a digit event with
a new program therefore removes queued prompt work without a separate cancel instruction. At most
one callback is outstanding per call; events that happen meanwhile queue and are delivered in
sequence with a current snapshot.

Session mode is specified as full duplex for actions that need not alternate with callbacks, such
as originating a call or acting on an external command. The embedded mode is intended to preserve
the same session semantics without a wire boundary.

These are contract rules tested by the interpreter and harness. They are not usable deployment
modes yet because all three binding adapters are unavailable.

## Failure policy

The configuration declares a callback timeout and an action for timeout, an unreachable app, and
4xx or 5xx responses. Depending on the condition, the policy may continue the current program,
hang up, or reject the call. The default preserves already-scripted work instead of ending an
active call merely because the next callback cannot be reached.

This policy is active in the current host for the one failure it can encounter without a binding:
the app is always unreachable. It does not mean the host has attempted an HTTP request, opened a
session, or loaded a handler.

See the [SDK overview](overview.md) for the implementation boundary and the supported alternatives
available today.
