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

`T-32` separates observation from policy. A bounded, non-blocking stream observes parsed inbound and
finalized outbound messages plus connection lifecycle, with loss counted. A narrower pre-transaction
policy may approve, reject or add application-owned headers before transaction keys, Digest, Via and
Content-Length are finalized. There is no arbitrary post-key mutator and no second target resolver.

## Exit

Concurrent reload tests observe only complete old or complete new identities; invalid replacements
leave the old identity active; established sessions survive; observer overload or failure cannot stop
the endpoint; and policy tests prove protected routing, sequencing and authentication fields cannot
be rewritten.
