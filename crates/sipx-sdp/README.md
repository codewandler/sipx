# sipx-sdp

SDP session descriptions (RFC 8866) and offer/answer negotiation (RFC 3264).

## What this is

A pure parser, serializer, and offer/answer engine. Unknown lines survive round trips, while typed
accessors and negotiation functions expose the parts of a session sipx understands.

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_sdp/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Deliberately absent

This crate performs no I/O and starts no media. Binding sockets, gathering network candidates, and
running the selected session belong to the media and call drivers.

## See also

- [`docs/specs/sdp-format-identity.md`](../../docs/specs/sdp-format-identity.md) — negotiated format identity.
- [`sipx-call`](../sipx-call/README.md) — the layer that applies negotiation to a call.
