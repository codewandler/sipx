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
  `Via`; a narrow standard-header allowlist and truly unknown extensions are accepted only after
  canonicalizing names, so the returned shape cannot rewrite protocol semantics or framing.
- A live, configured-size-bounded source-admission generation is read before UDP parsing and before
  an accepted stream enters TLS/WebSocket handshaking or SIP framing. Replacement and clear publish
  one complete generation.
  Connections admitted by an older generation remain admitted until they close; policy rotation is
  not retroactive connection revocation.

Source admission is a non-zero-limit address/prefix set, not a callback. Oversized replacement is a
typed refusal that preserves the current generation. That keeps the receive path bounded and
prevents a hostile population from creating per-source tasks, futures or map entries. Refusal counts
are shared atomics, like the existing endpoint counters. There is no arbitrary post-key mutator, no
second target resolver and no observer callback on the endpoint loop.

Source admission is earlier still. UDP source addresses are checked before parsing a datagram;
connection source addresses are checked before TLS/WebSocket handshake and stream framing. Its live
configuration is an atomically replaced bounded IP/prefix set, not an async application callback and
not a parsed-message hook. A refused source creates no per-source task or transaction and increments
an observable counter. Existing admitted connections keep the generation that admitted them; a
replacement governs new datagrams and new connections rather than retroactively tearing down work.

## Exit

Concurrent reload tests observe only complete old or complete new identities; invalid replacements
leave the old identity active; established sessions survive; observer overload or failure cannot stop
the endpoint; and policy tests prove protected routing, sequencing and authentication fields cannot
be rewritten.
Source-admission vectors additionally prove refusal before UDP parsing and before a stream handshake,
atomic live replacement, and survival of an already admitted connection across replacement.
