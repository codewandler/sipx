---
title: The contract, in short
description: sipx.app.v1 at a glance — the event and instruction vocabulary, the envelope, and the rule that makes barge-in compose.
---

# The contract, in short

:::caution Preview

`sipx.app.v1` is experimental. The normative version — with the envelope fields, failure
semantics, authentication and test vectors — is
[`docs/specs/app-contract.md`](https://github.com/codewandler/sipx/blob/main/docs/specs/app-contract.md);
this page is the tour.

:::

## An event

Every event carries a **full snapshot** of the call — your code never needs to remember state,
and a missed delivery cannot leave it permanently wrong:

```json
{
  "contract": "sipx.app.v1",
  "seq": 4,
  "at": "2026-07-28T09:15:04.221Z",
  "call": {
    "id": "b7c1…", "direction": "inbound", "state": "answered",
    "from": "sip:alice@example.com", "to": "sip:support@example.net",
    "media": { "encrypted": true, "on_hold": false, "muted": false }
  },
  "event": { "type": "call.dtmf", "digit": "5", "duration_ms": 160 }
}
```

Event types: `call.incoming` · `call.ringing` · `call.answered` · `call.dtmf` ·
`call.playback.finished` · `call.gather.finished` · `call.recording.finished` ·
`call.dial.finished` · `call.transfer.requested` · `call.transfer.progress` · `call.bridged` /
`call.unbridged` · `call.hold` / `call.resumed` · `call.ended`.

## A program

Your response is an ordered program. Every instruction has your `id`, echoed back on its
completion event:

```json
{
  "contract": "sipx.app.v1",
  "instructions": [
    { "id": "p1", "do": "play", "source": { "file": "welcome.wav" }, "interruptible": true },
    { "id": "g1", "do": "gather", "max": 4, "terminators": "#", "timeout_ms": 10000 }
  ]
}
```

Verbs: `answer` · `ring` · `reject` · `play` · `gather` · `record` · `send_dtmf` · `dial` ·
`bridge` / `unbridge` · `hold` / `resume` · `mute` / `unmute` · `transfer` (blind or attended)
· `accept_transfer` / `refuse_transfer` · `pause` · `tag` · `hangup`.

## The rule that makes it compose

In webhook mode, **a response replaces the whole pending program**. Respond to a `call.dtmf`
event with a new program and the queued prompt is gone — that is barge-in, without a cancel
API. At most one callback is outstanding per call; events that happen meanwhile queue and
arrive in order, each with a current snapshot.

Anything that needs to act at an arbitrary moment — coordinating two legs, killing a call from
the outside, originating calls — is **session mode**: same vocabulary over a WebSocket, no
alternation, plus `originate`.

## What happens when your code is down

Declared, not coded: per app you configure `timeout_ms` and what a timeout, a 5xx or an
unreachable endpoint each mean — keep going with the current program, hang up, or reject. The
default keeps an already-scripted call alive rather than killing it.
