# sipx-app

SIP application host with webhook, full-duplex session, and realtime audio bindings.

## What this is

This is the leaf application over the sipx stack. It reads a host document, binds the declared
listeners, answers calls through `sipx-call`, drives webhook applications through the contract
interpreter, serves authenticated full-duplex sessions, bridges configured G.711 calls to realtime
WebSocket sessions, and provides the deterministic fake-time harness used to exercise host policy.
The production webhook and session paths share `sipx-app-protocol::Interpreter` with that harness;
the realtime binding is a terminal call application with its own normative event contract.

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_app/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Binding boundary

Document-mode webhooks are implemented. Response bodies stay opaque until
`sipx-app-protocol::Interpreter` parses them; the host executes only effects the interpreter yields.
Authenticated full-duplex sessions can replace programs and originate calls when granted. The
realtime binding answers a routed call, keeps its negotiated PCMU or PCMA bytes encoded in both
directions, and emits one terminal JSON record from `sipx-host`. The embedded runtime and packaged
TypeScript SDK are not implemented, and the language-neutral wire contract remains Experimental.

## See also

- [`docs/designs/app-host.md`](../../docs/designs/app-host.md) — host architecture and phases.
- [`docs/specs/app-contract.md`](../../docs/specs/app-contract.md) — the application vocabulary.
- [`docs/specs/openai-realtime.md`](../../docs/specs/openai-realtime.md) — the realtime bridge
  contract and vectors.
