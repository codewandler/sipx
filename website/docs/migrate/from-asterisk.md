---
title: Migrating from Asterisk
description: An honest concept map — which Asterisk roles sipx covers today, which the SDK will cover, and which have no equivalent planned.
---

# Migrating from Asterisk

Asterisk is many products in one process: endpoints, a dialplan, media applications, queues,
voicemail, conferencing, and two generations of control API. sipx is one thing — the endpoint
side of SIP, built kernel-first — and an ecosystem growing around it. So the honest first
answer is:

**If you use Asterisk as a programmable way to place, answer and script calls, sipx covers a
useful part of that today and is building toward the rest. If you use it as a full PBX —
queues, voicemail, hunt groups — sipx is not a replacement, and this page will not pretend
otherwise.**

## Maps today / not yet

| In your Asterisk deployment | Goes to | Status |
|---|---|---|
| Originating calls programmatically | `sipx dial` / `sipx_call::dial` | **today** |
| Answering and scripting a call in code: play, record, DTMF in/out | `sipx-call` (Rust) | **today** |
| Endpoint registration (the `pjsip.conf` client side) | `sipx register` / `sipx-ua`, with digest, TLS, Outbound | **today** |
| Bridging two calls, conferencing several | `sipx-call` + `sipx-media` (Rust; bridge from the public API is being finished) | today / in progress |
| Transfers (blind and attended) | `sipx-call` REFER support | **today** |
| Call quality visibility | per-call loss, jitter, RTT, MOS estimate | **today** |
| The dialplan, and event/instruction control of a call from your own code | the [`sipx.app.v1` contract](../sdk/overview.md): events out, instructions in, webhook or session | **specified, experimental** — the host (`sipx-app`) is in development |
| Call event stream for monitoring | the same contract's event side | designed, in progress |
| Running handler scripts inside the server | the host's embedded TypeScript runtime | designed |
| Registrar for your phones | [sipx-clstr](https://github.com/codewandler/sipx-clstr) | early — in development |
| Queues, voicemail, parking, hunt groups | — | **no equivalent planned**; these become applications you build on the SDK, not features of sipx |
| Music on hold, TTS | — | not planned (playback of your own audio: today) |

## What migrating today looks like

The realistic near-term move is the **programmatic-calls** slice of an Asterisk deployment:

- **A notification dialer** — dial, play an announcement, collect a confirmation digit, record
  the outcome. Today: the CLI does the whole loop from a shell script (`--play`, `--dtmf`,
  `--record`, `--json`, exit codes), or `sipx-call` does it in Rust with full control.
- **A test harness** — place real calls against your systems and assert on what happened.
  Today, and it is one of the things sipx is built for: every claim it makes about itself is
  shell-assertable.
- **A simple IVR** — answer, prompt, gather, act. Today in Rust; without Rust once the SDK's
  host ships — that is exactly the first workload the
  [contract](../sdk/contract.md) is being proven against.

## Where your dialplan goes

Nowhere, verbatim — and that is deliberate. sipx's ecosystem does not have a configuration
language that grows into a programming language. The replacement shape is the
[SDK](../sdk/overview.md): your call logic lives in *your* code, in your language, receiving
typed events and returning instructions. What the dialplan expressed as pattern-matched
extensions becomes ordinary conditionals in a program you can test.

Until the SDK's host ships, call logic on sipx means Rust against `sipx-call`. If that is a
blocker rather than an option for you, the honest advice is: keep Asterisk for that role and
put sipx where it is strong today — the programmatic endpoints around it — then revisit when
the [SDK](../sdk/overview.md) leaves preview.

## What does not carry over

- Dialplan files, AGI/ARI/AMI integrations, and channel-driver configuration — the concepts
  map (events, instructions, endpoints), the artifacts do not.
- The all-in-one-process shape. sipx's ecosystem separates the kernel, the cluster platform
  and the application host on purpose; each is smaller than Asterisk and provable on its own.
- Features that are applications wearing a PBX costume — queues, voicemail. On this stack
  they are yours to build, with the SDK as the intended tool.
