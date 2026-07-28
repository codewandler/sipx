# sipx — vision & principles

This document states *why* sipx exists and the principles that decide how it's built. It is
the **tie-breaker** when a design choice is unclear: prefer the option that best serves the
north star and the principles below.

## What sipx is

sipx is a complete SIP and VoIP stack in Rust — signalling, media, a call framework, and a
scriptable softphone — for people who build telephony systems rather than merely speak the
protocol. It is aimed at the engineer who needs to place a real call from a test, run a
B2BUA that survives a carrier's malformed `Via`, or ship a voice product without inheriting
a C stack from 2004.

The defining idea is that **the protocol core does no I/O**. Parsing, transactions and
dialog state are pure state machines. Everything asynchronous is a thin driver over them.
Existing stacks tangle protocol logic with sockets and threads, which is exactly why their
hardest bugs — retransmission races, transaction mismatches, panics on hostile input —
are so hard to reproduce.

## North star

**Correct under adversarial input and adversarial timing, provably.** Not "passes a happy-path
call", but: every branch of the transaction machinery is reachable in a unit test, every parse
path is fuzzed, and no network peer can cause a panic. Interoperability follows from
correctness; performance is meaningless without it.

## Principles

1. **Sans-IO at the core.** If logic can be expressed as a function from inputs to outputs, it
   must be. Time enters as a fired-timer input, never as a clock read. Settles: where a piece
   of logic lives, and whether a test may use a real socket (it may not, below the transport
   layer).
2. **Malformed input is a value.** Hostile bytes produce typed errors, never panics and never
   `unsafe`. Settles: error handling style, and whether a shortcut that could index out of
   bounds is acceptable (it isn't).
3. **Own, don't share.** A call owns its media pipeline; data moves over channels. Settles
   the recurring "just put a mutex on the session" temptation — the answer is no, because a
   stalled leg must never block its peer.
4. **The spec precedes the code.** Every subsystem has a written contract in `docs/specs/`
   with RFC citations, state tables and test vectors. Settles what to do when the RFC is
   ambiguous: decide once, in the spec, with the rationale recorded.
5. **Cite primary sources.** Design rationale references RFCs or our own specs — never a
   third-party implementation. Settles how behaviour learned empirically gets documented:
   as a requirement with its own justification.
6. **Testable from a shell.** If a feature can't be asserted on from a script, it isn't
   finished. Settles the CLI's output design and why every layer has a loopback harness.

## Non-goals

- **A WebRTC stack.** SRTP for SIP interop, yes; a browser media engine, no.
- **Video.** The media layer is built for telephony audio. Video would compromise the
  latency and simplicity budget without serving the north star.
- **A configuration-driven PBX.** sipx is a library and a phone. Routing engines and dial
  plans are things you build *with* it.
- **Maximum feature count.** A smaller stack whose every path is tested beats a larger one
  whose edges are guesswork.
