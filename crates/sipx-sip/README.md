# sipx-sip

Sans-IO SIP core: messages, parser and transactions (RFC 3261).

## What this is

The protocol core as data and state machines. Bytes enter parsers, transaction events enter finite
state machines, and the results say what bytes or timers a driver should handle next.

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_sip/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Deliberately absent

This crate opens no socket, reads no clock, and starts no async runtime. Transports drive it;
dialogs, calls, and media live in higher layers.

## See also

- [`docs/specs/sip-message.md`](../../docs/specs/sip-message.md) — message parsing and preservation.
- [`docs/specs/sip-transaction.md`](../../docs/specs/sip-transaction.md) — transaction machines.
