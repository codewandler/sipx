---
title: The experimental contract
description: A tour of sipx.app.v1 as used by the implemented webhook and full-duplex session bindings.
---

# The experimental contract

:::caution Experimental wire contract

`sipx-host` uses this contract with document-mode webhooks and authenticated full-duplex sessions.
The Supported Rust vocabulary and interpreter are frozen for compatible v1 evolution, but the wire
shape remains Experimental and unfrozen and may change without a migration path. An embedded
handler and packaged language SDK are not implemented.

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

<!-- BEGIN generated:app-event-families -->
The contract currently carries these event types, grouped into families by their wire names:

- `call.incoming`
- `call.ringing`
- `call.early_media.started`
- `call.answered`
- `call.dtmf`
- `call.voice.started`
- `call.voice.ended`
- `call.voice.thresholds`
- `call.signal.metrics`
- `call.signal.silence`
- `call.playback.finished`
- `call.gather.finished`
- `call.recording.finished`
- `call.dial.finished`
- `call.leg.ended`
- `call.transfer.requested`
- `call.transfer.progress`
- `call.bridged`
- `call.unbridged`
- `call.dsp.activated`
- `call.dsp.configured`
- `call.dsp.bypassed`
- `call.dsp.restored`
- `call.dsp.removed`
- `call.dsp.refused`
- `call.hold`
- `call.resumed`
- `call.ended`

The [normative event table](https://github.com/codewandler/sipx/blob/main/docs/specs/app-contract.md#53-event-types) defines their fields, ordering, and emission rules; this generated inventory is only a route to that table.
<!-- END generated:app-event-families -->

Voice activity is `call.voice.started` and `call.voice.ended`, and it is **deterministic signal
analysis rather than recognition** — no speech model is loaded to produce it, so a host built
without a speech runtime still reports it. Each carries the side of the audio it was observed on,
the position in samples at the rate those samples are counted at, and an observation number that
orders one call's voice events. Which call it is about is the envelope's own `call.id`.

What those decisions are measured *against* is readable too. Every event's call snapshot carries a
`voice` member when detection is running on the call — the amplitude the activation predicate is
comparing against, the window and hangover it is compared over, and the calibration bounds when the
threshold is one that adapts — and `call.voice.thresholds` announces it once when detection starts
and again whenever calibration moved it, so nothing polls and a settled threshold is silent. Every
member is a sample count or an amplitude, counted at the rate the event names rather than in
wall-clock time; the surface carries no audio and no field audio could be rebuilt from, and a call
nobody asked for detection on has no `voice` member at all.

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
mute, transfer, pause, tag, hangup, and the three DSP operations below. A word in the contract is
not itself evidence that the current host or public call API can perform that operation end to end.

## Shaping the call's audio

Three instructions compose a bounded DSP chain on one direction of a call, and five
`call.dsp.*` events report what happened to it:

```json
{
  "contract": "sipx.app.v1",
  "instructions": [
    { "id": "d1", "do": "dsp", "direction": "outbound", "processors": [
        { "id": "sipx.gain", "shape": 0, "parameters": { "gain": { "ratio": 2000 } } },
        { "id": "sipx.low_pass", "shape": 0, "parameters": { "cutoff_hz": { "integer": 3400 } } } ] }
  ]
}
```

`dsp` sets the chain, `dsp_param` moves one stage's parameters against the generation the chain
was last reported at, and `dsp_remove` takes it away and waits for the graph to hold nothing. Each
resolves with exactly one event: its outcome, or `call.dsp.refused`. Two more arrive unasked —
`call.dsp.bypassed` when a stage stops contributing, and `call.dsp.removed` when a fail-closed
stage or a stopped session ends the chain.

**What an application can say is names, a shape and finite values, and nothing else.** A `dsp`
instruction has no field for a processor, a program, a callback, an execution profile, a deadline,
a failure action or a graph bound — those are not filtered out, they are not expressible, in the
same way that `play.source` has no URL. So `contains_overrun` on `call.dsp.activated`, which says
whether over-budget work in the chain can stall RTP, is something the host derives from the chain's
stages and reports; there is no instruction that sets it. Parameter values carry their kind
(`{"flag": …}`, `{"integer": …}`, `{"ratio": …}` in thousandths) because integers and ratios are
both JSON numbers and there is no floating-point parameter to fall back on.

Which processors exist is the host's registry, and every identifier that resolves is one of this
workspace's own. The normative rules — the closed schemas, the sample boundary a change lands on,
what each refusal leaves untouched — are
[`docs/specs/call-dsp-graph.md`](https://github.com/codewandler/sipx/blob/main/docs/specs/call-dsp-graph.md)
§10.

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
