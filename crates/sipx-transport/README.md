# sipx-transport

Async SIP transports: UDP, TCP, TLS, WebSocket, experimental QUIC, and RFC 3263 resolution.

## What this is

The I/O driver for `sipx-sip`. It owns sockets, connection reuse, target resolution, timers, and
capture while feeding received bytes and fired timers into the sans-I/O transaction core.

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_transport/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Deliberately absent

This crate does not decide registration, dialog, call, or media behavior. Those layers consume its
endpoint and incoming-message interfaces without moving their policy into the socket driver.

## See also

- [`docs/specs/sip-transport.md`](../../docs/specs/sip-transport.md) — transport and pooling rules.
- [`docs/specs/sip-quic.md`](../../docs/specs/sip-quic.md) — sipx's experimental QUIC mapping.
- [`sipx-sip`](../sipx-sip/README.md) — the core driven by these transports.
