---
title: SDK overview
description: Experimental application hosting — what sipx-host does now and which customer-code bindings are still unavailable.
---

# SDK overview

:::caution Experimental

The application host and `sipx.app.v1` contract may change without a migration path. There is no
supported language-neutral SDK or callback binding today.

:::

## What exists today

`sipx-host` is a real process. Given a host configuration, it binds the first declared SIP
listener, admits incoming invitations under that configuration, answers or refuses them according
to the declared failure policy, and carries answered calls until the caller ends them. It also
answers the out-of-dialog requests a call listener must handle.

The surrounding implementation includes:

- a parser and validator for host configuration;
- the `sipx.app.v1` Rust types and JSON wire format in `sipx-app-protocol`;
- a sans-I/O instruction interpreter;
- a deterministic harness for contract events, instructions, timing, retries, and failure policy;
- a host binary whose real SIP path is exercised by tests.

That is enough to prove that the host can run and answer a call. It is not yet enough for customer
code to control that call.

## What is unavailable

None of the customer-code bindings is implemented:

| Intended mode | Boundary | Status |
|---|---|---|
| Webhook | The host sends an event document to an HTTP endpoint and applies its returned program | Unavailable |
| Session | A controller exchanges events and instructions over a full-duplex connection | Unavailable |
| Embedded handler | The host runs a handler in-process | Unavailable |

The host accepts configuration that describes these modes because the configuration and failure
semantics are implemented first. It treats the app as unreachable at runtime. The configured
`on_unreachable` action therefore decides whether an incoming call is rejected, answered and held,
or answered and then hung up. A successful host start does not prove that a configured callback is
being invoked.

There is also no TypeScript package, no WebSocket session server, no webhook delivery client, and
no embedded TypeScript engine in this repository today.

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
The [contract tour](contract.md) describes its envelope and vocabulary while keeping the missing
runtime boundary explicit.

## What to use now

Use the [CLI](../reference/cli.md) for shell automation and bounded call tasks. Use
[`sipx-call`](../guides/as-a-library.md) when a Rust application needs to own call behavior. Do not
build a deployment that depends on webhook, session, or embedded callbacks until those bindings
exist and this page no longer labels them unavailable.

The host is not intended to become a proxy, registrar, routing engine, voicemail system, or
configuration-driven exchange. Those are separate roles; see
[Integrate with an existing SIP system](../guides/integrate-existing-system.md).
