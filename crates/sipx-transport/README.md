# sipx-transport

Async SIP transports: UDP, TCP, TLS, WebSocket, experimental QUIC, RFC 3263 resolution, and bounded
live endpoint operations.

## What this is

The I/O driver for `sipx-sip`. It owns sockets, connection reuse, target resolution, timers, and
capture while feeding received bytes and fired timers into the sans-I/O transaction core.
The same bounded, redacted capture path can send best-effort HEP3 datagrams to an operator-owned
collector without moving collector I/O into a SIP timer path.

An embedding host can atomically rotate the identity used by new TLS and secure-WebSocket
handshakes, observe parsed messages and connection transitions through one bounded non-blocking
receiver, install an immutable pre-transaction request policy, and replace a bounded source-prefix
admission generation before parsing or handshake work. Existing connections are not silently
renegotiated or reclassified.

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_transport/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Deliberately absent

This crate does not decide registration, dialog, call, or media behavior. Those layers consume its
endpoint and incoming-message interfaces without moving their policy into the socket driver.

## See also

- [`docs/specs/sip-transport.md`](../../docs/specs/sip-transport.md) — transport and pooling rules.
- [`docs/specs/observability-export.md`](../../docs/specs/observability-export.md) — HEP3 export and
  application-owned RTCP quality hooks.
- [`docs/designs/live-endpoint-policy.md`](../../docs/designs/live-endpoint-policy.md) — ownership and
  limits of identity rotation, observation, request policy, and source admission.
- [`docs/specs/sip-quic.md`](../../docs/specs/sip-quic.md) — sipx's experimental QUIC mapping.
- [`sipx-sip`](../sipx-sip/README.md) — the core driven by these transports.
