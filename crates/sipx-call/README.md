# sipx-call

Call framework: dial, answer, couple dialogs, play, record, send DTMF, and transfer.

## What this is

The layer where SIP dialogs, SDP negotiation, and media sessions become a call. It owns the
single-call lifecycle, can couple two dialogs with optional bridged media, and exposes the
operations and typed events an application uses.

Confirmed, quiescent dialogs can be captured as bounded versioned bytes and attached to a fresh
endpoint and an already-created matching media session. This is protocol-state persistence, not a
serialized runtime: sockets, transactions, tasks, clocks, credentials and media keys are excluded.
The host owns durable storage, authorization, encryption at rest, distribution and single-owner
selection.

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_call/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Deliberately absent

A coupling still terminates media at sipx even when no bridge is attached, so the signalling-only,
off-media role is absent. Multi-party call bridging and conferencing are absent too; the media crate
can mix sessions an application owns, but a `Call` keeps its media session private.

## See also

- [`docs/specs/call-dispatch.md`](../../docs/specs/call-dispatch.md) — routing one endpoint to many calls.
- [`docs/specs/call-coupling.md`](../../docs/specs/call-coupling.md) — driving two dialogs as one call.
- [`docs/specs/sip-auth.md`](../../docs/specs/sip-auth.md) — bounded 401/407 retries for outbound calls.
- [`docs/specs/dialog-persistence.md`](../../docs/specs/dialog-persistence.md) — bounded confirmed-dialog capture and runtime attachment.
- [`sipx-media`](../sipx-media/README.md) — the media sessions owned by calls.
