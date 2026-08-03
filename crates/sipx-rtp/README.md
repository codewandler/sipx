# sipx-rtp

RTP and RTCP packet handling, sequencing, jitter buffering, quality statistics and SRTP (RFC 3550).

## What this is

The packet and stream-state layer for telephony media. It parses hostile datagrams, tracks RTP
sequence space, buffers jitter, produces RTCP statistics, carries telephone events, and protects
packets with SRTP.

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_rtp/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Deliberately absent

This crate opens no socket, reads no SDP, and owns no call lifecycle. Network pacing and NAT
behavior belong in `sipx-media`; negotiation belongs in `sipx-sdp` and `sipx-call`.

## See also

- [`sipx-media`](../sipx-media/README.md) — the asynchronous session driver.
- [`docs/specs/srtp.md`](../../docs/specs/srtp.md) — secure packet processing and keying.
