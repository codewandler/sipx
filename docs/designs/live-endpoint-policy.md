# Design: live endpoint policy

**Status:** accepted · **Pillar:** Transport · **Epic:** `live-endpoint-policy` · **Stories:** T-31,
T-32

## Why

A long-running endpoint has two operational seams that currently require replacement or a fork:
rotating its TLS server identity and observing or applying narrow policy to traffic at typed lifecycle
points. Both are security boundaries. Reloading half an identity can break or weaken handshakes;
an unconstrained message mutator can invalidate transaction keys, authentication and framing after
the stack has committed to them.

## Approach

`T-31` accepts a fully parsed certificate/key identity, validates it before publication, and swaps it
atomically for new TLS and WSS handshakes. Established connections retain the identity they began
with. File watching, signals and secret-store clients remain host concerns.

`T-32` separates three seams that have different timing and trust boundaries.

- A bounded, non-blocking stream observes parsed inbound and finalized outbound messages plus
  connection lifecycle, with loss counted. Observation never runs application code on the driver.
- A pre-transaction request policy receives an immutable request and may approve, reject or return
  application-owned headers. It runs in the calling task, before the transport adds its branch and
  `Via`; the returned shape cannot rewrite identity, routing, authentication or framing fields.
- A live source-admission generation is read before UDP parsing and before an accepted stream enters
  TLS/WebSocket handshaking or SIP framing. Replacement and clear publish one complete generation.
  Connections admitted by an older generation remain admitted until they close; policy rotation is
  not retroactive connection revocation.

Source admission is an address/prefix set, not a callback. That keeps the receive path bounded and
prevents a hostile population from creating per-source tasks, futures or map entries. Refusal counts
are shared atomics, like the existing endpoint counters. There is no arbitrary post-key mutator, no
second target resolver and no observer callback on the endpoint loop.

## Exit

Concurrent reload tests observe only complete old or complete new identities; invalid replacements
leave the old identity active; established sessions survive; observer overload or failure cannot stop
the endpoint; and policy tests prove protected routing, sequencing and authentication fields cannot
be rewritten.
Source-admission vectors additionally prove refusal before UDP parsing and before a stream handshake,
atomic live replacement, and survival of an already admitted connection across replacement.
