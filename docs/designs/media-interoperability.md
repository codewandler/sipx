---
id: media-interoperability
---

# Media interoperability closure

**Status:** proposed · **Pillar:** Media · **Epic:** `media-interoperability` ·
**Review:** [external functionality and usability review](../reviews/extern-2026-08-06T01-18-47+02-00-full-sweep.md)
findings 3, 9 and 10 · **Stories:** `M-69`, `M-70`, `M-71`

## Problem

The media primitives exist, but three product-boundary compositions fail: an unacceptable initial
offer becomes local error text without a final SIP response; a multiplexed browser offer is refused
when it also carries an unused RTCP candidate; and negotiated RFC 4733 digits sent through scenario
do not surface as receive events. All three turn supported protocol vocabulary into peer-visible
failure at the adapter between SDP, call control and media runtime.

## Direction

- RFC 3264 offer/answer errors remain typed through call control. An unacceptable initial INVITE is
  answered with RFC 3261's final failure response (488 for an unsupported session), including the
  normal server-transaction and To-tag behavior, before the local command reports failure.
- RFC 5761 multiplexing requires one nominated runtime component. A remote description may carry
  candidate information for an unused second component without proving that multiplexing is
  absent. Component-one viability and `a=rtcp-mux` decide the profile; component two is never
  nominated or given a second socket.
- RFC 4733 payload identity comes from negotiated SDP, not payload number 101 by assumption.
  Start/continuation/end packets produce one ordered typed digit event with bounded duplicate and
  reordering handling, and the event crosses the existing call/scenario stream without a shadow
  decoder.

## Verification

Each story starts with a wire or byte-vector reproduction from the review. The unacceptable offer
proof asserts a final response and caller classification, the multiplexing proof pins exact SDP
with both candidate components and exercises the independent browser harness, and the DTMF proof
sends a digit sequence between two scenario-controlled calls and observes the typed remote events.

## Boundaries and exit

This epic does not widen the browser profile, add a second RTCP socket, or add in-band tone
detection. It closes already-advertised behavior. It is done when capability mismatch is explicit
on the wire, valid multiplexed offers reach nominated protected media, and negotiated telephone
events are observable end to end without weakening malformed-input or resource bounds.
