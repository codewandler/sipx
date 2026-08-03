# sipx-call

Call framework: answer and dial calls with playback, recording, DTMF and transfer.

## What this is

The layer where SIP dialogs, SDP negotiation, and media sessions become a call. It owns the
single-call lifecycle and exposes the operations and typed events an application uses.

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_call/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Deliberately absent

A `Call` does not yet expose multi-party bridging or conferencing. That is a call-layer capability
rather than a workaround for an application to build around private media state.

## See also

- [`docs/specs/call-dispatch.md`](../../docs/specs/call-dispatch.md) — routing one endpoint to many calls.
- [`docs/specs/sip-auth.md`](../../docs/specs/sip-auth.md) — bounded 401/407 retries for outbound calls.
- [`sipx-media`](../sipx-media/README.md) — the media sessions owned by calls.
