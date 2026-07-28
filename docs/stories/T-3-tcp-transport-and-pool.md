---
id: T-3
title: Implement the TCP transport with connection pooling and reuse
pillar: Signalling
status: done
priority:
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport]
note:
---

# Implement the TCP transport with connection pooling and reuse

## Goal
Add stream transport: framed message reading, a connection pool that reuses established
connections, and correct behaviour when a connection dies mid-transaction.

## Acceptance
- [x] Messages are framed with the incremental parser from `S-4`; a message split across TCP
      segments is assembled correctly.
- [x] Connections are pooled by (transport, remote address) and reused for subsequent
      requests per RFC 5923.
- [x] An inbound connection is reused for responses and for in-dialog requests back to the
      same peer.
- [x] On connection close, transactions bound to it are terminated with a transport error
      rather than left to time out.
- [x] Idle connections are closed after a configurable timeout; the pool has a bounded size
      and a documented eviction policy.
- [x] Failing-first test: `tcp_message_split_across_segments_is_assembled`.

## Progress
- Done. `crates/sipx-transport/src/tcp.rs` (the pool) wired into the endpoint loop.
- The pool tracks how each connection came to exist. A response always goes back over the
  connection its request arrived on — RFC 5923, and the only thing that works when the peer is
  behind a NAT. An outbound *request* is different: reusing an inbound connection for one is
  how a peer that connected to you gets your traffic routed through it, so that stays off
  unless `reuse_inbound_for_outbound` is set. The distinction is made at the point of sending,
  by whether the message is a response.
- The dial does not block the endpoint loop. A peer that black-holes SYN takes about two
  minutes to fail, and the loop that would have waited also owns every transaction timer —
  waiting there would stop retransmissions for calls that have nothing to do with that peer.
- The fault-injection loopback deferred from `T-2` is still not built. The real-socket tests
  cover the ground it was meant to: one-byte-at-a-time segmentation, two messages in one
  segment, a body arriving after its headers, and a connection dropped mid-transaction.

## Notes
- Connection reuse is a security-relevant default; the spec decision from `T-1` governs.
