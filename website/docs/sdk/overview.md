---
title: SDK overview
description: Where "build call behaviour without writing Rust" is headed — call events out, instructions in, one contract with webhook, session and embedded runtimes.
---

# SDK overview

:::caution Preview

The SDK is **specified, not shipped**. This page describes the direction and what exists
today, honestly labeled. The wire contract is experimental and may change until two real
applications run against it.

:::

## The idea

Everything sipx can do on a call — answer, play, collect digits, dial, bridge, transfer, mute,
hang up — expressed as **data** instead of Rust:

- The server sends your code an **event** (a call arrived, a digit was pressed, playback
  finished), carrying a full snapshot of the call.
- Your code returns **instructions** — an ordered program: *answer, play `welcome.wav`, gather
  four digits, dial the person they asked for, bridge*.

Your code never touches SIP, never parses a message, and is written in whatever runs where you
deploy it. Three ways of running the same contract:

| Mode | Your code is | Status |
|---|---|---|
| **Webhook** | an HTTP endpoint returning instruction documents | specified |
| **Session** | a process holding a WebSocket (or pipe), free to act mid-call at any time | specified |
| **Embedded TypeScript** | a `.ts` handler the server runs in-process | designed |

The TypeScript SDK's handler API is the same in session and embedded mode — a handler is
portable between "my own Node service" and "a file the server loads" without code changes.

## What is real today

- The contract is written down as a normative spec with test vectors:
  [`docs/specs/app-contract.md`](https://github.com/codewandler/sipx/blob/main/docs/specs/app-contract.md)
  — see [the contract summary](contract.md) for the short version.
- The kernel work it needs (a call event stream, multi-call serving, playback control, mute,
  bridging from the public API) is designed and tracked in
  [the app-sdk design](https://github.com/codewandler/sipx/blob/main/docs/designs/app-sdk.md).
- The host — the server that terminates calls and runs your handlers — is the `sipx-app`
  crate in this repository, in development; the crate exists, the code is on the board.

Until then, the scriptable surface that ships today is the **CLI**: every command speaks
`--json` and exits with a distinct code per outcome, which is enough for real automation —
dialers, monitors, test harnesses — from any language that can spawn a process. See the
[CLI reference](../reference/cli.md).

## What it will never do

No routing between endpoints, no registration control for other people, no raw SIP header
access, and no TTS verb. Dial plans and routing engines remain things you build *with* sipx —
the contract moves call behaviour across the language boundary; it does not turn sipx into a
configuration-driven PBX.
