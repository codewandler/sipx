# Design: SIP core (sans-IO)

**Status:** proposed · **Pillar:** Signalling · **Epic:** `sip-core` ·
**Stories:** S-1 … S-8, X-2

## Why

Everything above this layer inherits its correctness properties. SIP's genuinely hard parts —
retransmission timing, transaction matching, hostile input — are hard *to test* in most
implementations because protocol logic is entangled with sockets and task scheduling. Timing
bugs then surface as flaky integration tests, which get retried rather than fixed.

Separating the protocol from its I/O removes that entanglement. It is the decision the rest of
the codebase is organized around, so it is made here, once.

## Approach

`sipx-sip` is a pure state machine. It has no async runtime, opens no sockets and reads no
clock.

```
Input  ::= Received { data, source, transport }
         | TimerFired { id }
         | AppRequest { ... }

Output ::= Send { data, destination, transport }
         | SetTimer { id, duration }
         | ClearTimer { id }
         | Event { ... }          // delivered to the application
```

The async layer (`sipx-transport`) owns the sockets and the timer wheel; it feeds inputs in and
executes outputs. A test drives the same machine with a vector of inputs and asserts on the
outputs — no runtime, no sleeps.

Structure, bottom-up:

1. **Primitives** (`S-2`) — `Uri`, `HeaderName`, parameter lists. RFC 3261 §19 equivalence,
   not string equality.
2. **Messages** (`S-1`, `S-3`) — a parsed message holds the original bytes plus a header
   index; typed access is lazy. Unknown headers keep their original bytes, spelling and
   order.
3. **Parser** (`S-4`) — one implementation serving both datagram and stream framing, driven
   incrementally so a message split at any byte offset parses identically.
4. **Builders** (`S-5`) — construction is typed, so CRLF injection is unrepresentable rather
   than validated against.
5. **Transactions** (`S-6`, `S-7`) — the four FSMs of RFC 3261 §17, specified as state tables
   and implemented from them.
6. **Transaction layer** (`S-8`) — matching, stores, cleanup, and delivery of anything
   unmatched.

## Alternatives considered

- **Async throughout, a task per transaction.** Simpler to write and closer to how the RFC
  reads. Rejected: it makes timing behaviour untestable without a clock, and pushes every
  retransmission bug into flaky integration tests.
- **`nom` or a parser-combinator crate.** Attractive for the grammar. Rejected for the
  message parser: incremental stream framing and byte-exact passthrough of unknown headers
  both want an index over the original buffer, which cuts against combinator ergonomics. May
  still be reconsidered for self-contained sub-grammars.
- **Owned `String` headers.** Far simpler. Rejected: forwarding then reserializes, losing
  byte-exactness, and allocates per header on every proxied message.

## Risks & open questions

- **Ergonomics.** A sans-IO core can be unpleasant to consume directly. Mitigation:
  applications are expected to use `sipx-ua`, not this crate. If the UA layer turns out
  awkward, that is a signal the input/output vocabulary is wrong — revisit here, not there.
- **Zero-copy vs. mutation.** Messages that are modified in flight (adding a `Via`) need a
  representation that supports edits without losing passthrough for untouched headers. `S-1`
  must settle this explicitly.
- **Timer identity.** Timer IDs must be unique across transactions and survive transaction
  termination without firing into a dead machine. `S-6` settles the scheme.

## Acceptance / done

Every RFC 4475 valid message round-trips byte-exactly; every invalid one is rejected with a
specific typed error. Every row of the four transaction state tables is exercised by a test.
The parser fuzzes clean. No part of the crate depends on `tokio` or reads a clock.
