---
title: The experimental contract
description: A tour of sipx.app.v1 as used by the implemented webhook and full-duplex session bindings.
---

# The experimental contract

:::caution Experimental wire contract

`sipx-host` uses this contract with document-mode webhooks and authenticated full-duplex sessions.
The Rust vocabulary and interpreter are Supported under the pre-1.0 policy, but the wire shape may
change without a migration path. An embedded handler and packaged language SDK are not implemented.

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

The vocabulary covers incoming, ringing, answered, and ended calls; DTMF; voice activity; signal
metrics; playback, gather, recording, and dial completion; transfer progress; bridge state; and hold
state.

Voice activity is `call.voice.started` and `call.voice.ended`, and it is **deterministic signal
analysis rather than recognition** — no speech model is loaded to produce it, so a host built
without a speech runtime still reports it. Each carries the side of the audio it was observed on,
the position in samples at the rate those samples are counted at, and an observation number that
orders one call's voice events. Which call it is about is the envelope's own `call.id`.

Signal metrics are `call.signal.metrics` and `call.signal.silence`, from the same deterministic
analysis: level, clipping and silence over an exact stretch of the call's audio, each report naming
the measurement run, the samples and windows it covers, and the position it starts at. They describe
what the audio **contained**; packet loss, jitter, round-trip time and the MOS estimate describe how
it was **delivered**, live on the media stack's own RTP/RTCP surface, and neither substitutes for the
other.

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

Session mode is full duplex for actions that need not alternate with callbacks, such
as originating a call or acting on an external command. The embedded mode is intended to preserve
the same session semantics without a wire boundary.

The host implements both rules: document-mode webhook responses replace the pending program, and a
session controller may send correlated replacement documents without waiting for a callback.
Session calls remain pinned to one authenticated connection, and an app granted `originate` may
place a call through that connection. Embedded mode remains an intended carrier, not a shipped one.

## Failure policy

The configuration declares a callback timeout and an action for timeout, an unreachable app, and
4xx or 5xx responses. Depending on the condition, the policy may continue the current program,
hang up, or reject the call. The default preserves already-scripted work instead of ending an
active call merely because the next callback cannot be reached.

This policy is active in the current host. Webhook connection, timeout, and HTTP failures are fed
to the interpreter; session loss applies `on_unreachable` independently to every pinned call. An
embedded handler cannot be selected as a working binding because no embedded runtime is shipped.

See the [application host overview](overview.md) for the implementation boundary and the supported
alternatives available today.
