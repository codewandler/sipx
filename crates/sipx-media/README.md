# sipx-media

Media sessions: RTP/RTCP sockets bound to negotiated SDP with NAT handling, bridging and
conferencing.

## What this is

The asynchronous media driver: paced RTP sending, buffered receiving, RTCP, symmetric RTP, secure
media contexts, ICE, and workers that bridge or mix sessions an application owns.

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_media/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Deliberately absent

This crate does not own SIP calls or decide which media policy a call offers. Its DTLS handshake and
multi-party workers are lower-level mechanisms until the call layer selects or owns them.

## See also

- [`docs/designs/media.md`](../../docs/designs/media.md) — ownership, timing, and wait rules.
- [`sipx-rtp`](../sipx-rtp/README.md) — the sans-I/O packet layer below sessions.
