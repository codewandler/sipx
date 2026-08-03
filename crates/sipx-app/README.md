# sipx-app

Experimental SIP application host, configuration reader, and deterministic contract harness
(host process available; callback bindings not yet implemented).

## What this is

This is the leaf application over the sipx stack. It reads a host document, binds the declared
listeners, answers calls through `sipx-call`, and provides the deterministic fake-time harness used
to prove host policy before a real binding performs I/O.

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_app/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Deliberately absent

The host does not yet call customer code. Webhook documents, full-duplex sessions, and the embedded
runtime are separate host phases; an unreachable application follows the configured failure policy
instead of being simulated.

## See also

- [`docs/designs/app-host.md`](../../docs/designs/app-host.md) — host architecture and phases.
- [`docs/specs/app-contract.md`](../../docs/specs/app-contract.md) — the application vocabulary.
