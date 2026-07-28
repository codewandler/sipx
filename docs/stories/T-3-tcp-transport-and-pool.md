---
id: T-3
title: Implement the TCP transport with connection pooling and reuse
pillar: Signalling
status: backlog
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
- [ ] Messages are framed with the incremental parser from `S-4`; a message split across TCP
      segments is assembled correctly.
- [ ] Connections are pooled by (transport, remote address) and reused for subsequent
      requests per RFC 5923.
- [ ] An inbound connection is reused for responses and for in-dialog requests back to the
      same peer.
- [ ] On connection close, transactions bound to it are terminated with a transport error
      rather than left to time out.
- [ ] Idle connections are closed after a configurable timeout; the pool has a bounded size
      and a documented eviction policy.
- [ ] Failing-first test: `tcp_message_split_across_segments_is_assembled`.

## Progress
- Not started.

## Notes
- Connection reuse is a security-relevant default; the spec decision from `T-1` governs.
