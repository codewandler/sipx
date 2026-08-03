# sipx-app

SIP application host with document-mode webhooks, configuration, and a deterministic contract
harness.

## What this is

This is the leaf application over the sipx stack. It reads a host document, binds the declared
listeners, answers calls through `sipx-call`, drives webhook applications through the contract
interpreter, and provides the deterministic fake-time harness used to exercise host policy. The
production document path uses `sipx-app-protocol::Interpreter`; the older public harness still has
a provisional instruction runner whose migration to that interpreter remains open in `A-2`.

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_app/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Binding boundary

Document-mode webhooks are implemented. Response bodies stay opaque until
`sipx-app-protocol::Interpreter` parses them; the host executes only effects the interpreter yields.
Full-duplex sessions and the embedded runtime remain separate host phases.

## See also

- [`docs/designs/app-host.md`](../../docs/designs/app-host.md) — host architecture and phases.
- [`docs/specs/app-contract.md`](../../docs/specs/app-contract.md) — the application vocabulary.
