---
title: Application host overview
description: Application hosting through webhooks, full-duplex sessions, and realtime audio, plus the language-neutral contract's experimental boundary.
---

# Application host overview

:::caution Contract stability

The Rust host surfaces are Supported under sipx's pre-1.0 policy: incompatible changes receive
migration guidance, but APIs are not frozen. The `sipx.app.v1` wire contract remains Experimental
and may change without a migration path. There is no packaged language-specific SDK today.

:::

## What exists today

`sipx-host` is a real process. Given a host configuration, it binds a configured SIP listener and
application-session listener, admits incoming invitations under that configuration, and serves each
call through the selected application binding. It also answers the out-of-dialog requests a call
listener must handle.

The surrounding implementation includes:

- a parser and validator for host configuration;
- the `sipx.app.v1` Rust types and JSON wire format in `sipx-app-protocol`;
- a sans-I/O instruction interpreter;
- a deterministic harness for contract events, instructions, timing, retries, and failure policy;
- an HMAC-signed, bounded document-mode webhook client;
- a bearer-authenticated, bounded full-duplex session server with call pinning and origination;
- a bounded G.711 realtime bridge with named-secret authentication and barge-in;
- a host binary whose real SIP and application paths are exercised by tests.

Webhook and session controllers can drive real calls. Both feed the same interpreter, so callback
failure, replacement, and call effects have one implementation rather than separate meanings per
transport.

## Binding status

| Mode | Boundary | Status |
|---|---|---|
| Webhook | The host sends an event document to an HTTP endpoint and applies its returned program | Implemented |
| Session | An authenticated controller exchanges events and instructions over a full-duplex connection and may originate calls when granted | Implemented |
| Realtime | The host answers one routed call and passes its negotiated PCMU/PCMA bytes to one configured realtime WebSocket session | Implemented |
| Embedded handler | The host runs a handler in-process | Not implemented |

Document mode permits one outstanding callback per call. A successful response replaces the
pending instruction program. Session mode multiplexes calls, accepts replacement programs without
request/response alternation, and pins each call to one live session for its lifetime. Queue and
connection limits are finite; a dead or overloaded session applies each pinned call's configured
`on_unreachable` policy.

The session listener accepts cleartext WebSocket on a loopback or protected private network; put a
TLS terminator in front of it for a public network. There is no TypeScript package, subprocess
binding, embedded runtime, or embedded TypeScript engine in this repository today.

A realtime app is declared under `[app.<name>]` with `binding = "realtime"`, `endpoint`, `model`,
`instructions`, and `api_key_secret`. The document contains only the secret's name. Running
`sipx-host host.toml` is the one command that answers routed calls; each completed bridge writes a
JSON line naming `codec`, `packet_duration_ms`, and `session_outcome`.

The default test matrix proves the complete bridge contract against a deterministic loopback peer,
including authentication refusal, both audio directions, barge-in, stalls, close/reset behavior and
bounded cleanup. The credentialed live-endpoint interoperability proof has not yet been recorded,
so the implemented binding is not presented as evidence that the external service still matches
the observed contract.

## The intended model

The contract expresses call behavior as data:

- The host emits an event such as an incoming call, digit, or completed playback, with a snapshot
  of the call.
- Customer code returns an ordered instruction program such as answer, play, gather digits, dial,
  bridge, or hang up.
- Each binding carries the same event and instruction vocabulary; it must not add a private side
  channel to SIP messages or process state.

The normative contract is
[`docs/specs/app-contract.md`](https://github.com/codewandler/sipx/blob/main/docs/specs/app-contract.md).
The [contract tour](contract.md) describes its envelope, vocabulary, implemented bindings, and
remaining embedded-runtime boundary.

## What to use now

Use the [CLI](../reference/cli.md) for shell automation and bounded call tasks. Use
[`sipx-call`](../guides/as-a-library.md) when a Rust application needs to own call behavior in
process. Use webhook or session mode when a separately deployed controller can accept the
Experimental wire contract. Do not plan around an embedded handler or packaged TypeScript SDK;
neither exists.

The host is not intended to become a proxy, registrar, routing engine, voicemail system, or
configuration-driven exchange. Those are separate roles; see
[Integrate with an existing SIP system](../guides/integrate-existing-system.md).
