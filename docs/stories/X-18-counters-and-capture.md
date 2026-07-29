---
id: X-18
title: Count what the stack discards, and capture what it sends
pillar: Build
status: ready
priority: 10
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport, sipx-media, sipx-testkit]
note: M12 · nothing leaves the process but tracing; T-19 adds the first counter and has nowhere to put it
---

# Count what the stack discards, and capture what it sends

## Goal
Make a running sipx observable from outside: every discard counted and readable, and the signalling
it exchanged recoverable as a capture that can be attached to a bug report.

## Acceptance

**Counters**
- [ ] The endpoint exposes a counter snapshot — a plain type read through the handle, next to
      `Handle::outstanding` — covering at least: requests and responses in and out per transport,
      requests shed (`T-19`), responses that matched no client transaction (`T-18`), parse failures,
      retransmissions sent, and transactions timed out per timer.
- [ ] sipx depends on **no** metrics library. Counters are read through a snapshot or a trait, and the
      exporter is the application's choice. A stack that picks an exposition format picks it for every
      user of the library, and it is the one decision here that cannot be undone later.
- [ ] Media counters join them rather than living apart: `M-10` already computes quality statistics,
      and what is missing is that they are per-session and unreachable in aggregate.
- [ ] Every existing `let _ = …` discard and every `tracing::debug!("dropped …")` in the signalling
      path has a counter, and a test enumerates them so a new discard added without one is caught. A
      silent drop is the failure this whole story exists to end, and `T-19` fixes only the first one.

**Capture**
- [ ] Signalling can be recorded to a file: the messages the endpoint sent and received, with a
      timestamp, the transport, and both addresses. Bodies included — an SDP negotiation that went
      wrong is unreadable without them.
- [ ] The format is one standard packet-analysis tooling reads, and *which* format is the story's
      choice, recorded with its reason rather than left implicit.
- [ ] Capture is off by default and costs nothing when off. It is opt-in per endpoint, and enabling it
      must not change message ordering or timing — an observation that perturbs a retransmission race
      is worse than no observation.
- [ ] TLS, WSS and any future encrypted transport capture the *decrypted* messages, since capturing
      ciphertext from inside the process would be strictly worse than capturing outside it. The story
      says plainly that the resulting file contains credentials and call content.
- [ ] `sipx` gains a way to turn it on from the command line, because the
      [vision](../vision.md)'s "testable from a shell" is what makes this usable in an incident rather
      than only in a test.
- [ ] Failing-first test: `a_shed_request_and_an_unmatched_response_both_appear_in_the_counter_snapshot`.

## Progress
- Not started. The stack emits `tracing` and nothing else: no counter leaves the process, and there is
  no metrics or capture dependency in any crate manifest.

## Notes
- `T-19` is the story that makes this urgent rather than nice: its acceptance requires a shed request
  to be "counted, and the count […] reachable from outside the endpoint". That is one counter reached
  one way; this is the general case, and doing the general case second is the right order because
  `T-19` is a live fault.
- The counter list above is not a wish list. Each entry is a place where the current code loses
  information that a support case would need: which transport, how many, and when it started.
- Deliberately not in scope: tracing spans across the whole stack, and any push-based export. Both are
  the application's to build once the numbers exist.
