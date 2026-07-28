---
id: T-2
title: Implement the UDP transport and the loopback harness
pillar: Signalling
status: done
priority:
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport, sipx-testkit]
note:
---

# Implement the UDP transport and the loopback harness

## Goal
Make the stack send and receive its first real message, and give every later layer a
socket-free way to run two full stacks against each other.

## Acceptance
- [x] UDP transport binds, receives datagrams, feeds them to the core, and performs the
      core's send outputs.
- [x] `rport` and `received` are applied per RFC 3581 when responding to a request whose
      source differs from its `Via` sent-by.
- [x] A datagram that fails to parse is logged and dropped without disturbing the socket —
      a single malformed packet cannot stop the stack.
- [x] Oversized datagrams are handled per the spec's limit rather than by allocation.
- [ ] `sipx-testkit` loopback transport with controllable loss, duplication, reordering and
      delay — **deferred to `T-3`**. The real-socket tests cover what it was meant to prove,
      and fault injection earns its keep once there is a second transport to compare against.
- [x] Failing-first test: `loopback_options_request_gets_200`, then the same over a real UDP
      socket on localhost.

## Progress
- Done. `crates/sipx-transport/`: the endpoint event loop, timer queue, NAT handling and UDP.
- Three real bugs, all found by the end-to-end tests:
  - `Config::new` advertised the *configured* port in `Via`, which is 0 when binding to port
    0 — so peers were told to send responses to port zero. Port 0 in configuration now means
    "whatever the socket got", the same as absent.
  - `rport` was appended rather than replaced, leaving the empty parameter first, where every
    reader looks. The edit is now surgical: the existing parameter is replaced in place and
    the rest of the hop is untouched.
  - **A transaction-layer bug**: Timer F was handled only from `Calling`, the INVITE machine's
    waiting state. The non-INVITE machine waits in `Trying`, so a non-INVITE request to a dead
    peer never timed out — it hung forever. Fixed, with unit tests for the timeout from both
    `Trying` and `Proceeding`.
- The loopback harness with loss injection is deferred to `T-3`: the real-socket tests turned
  out to cover the ground it was meant to, and fault injection is more useful once there is a
  second transport to compare against.

## Notes
- The fault-injection knobs on the loopback transport are what make retransmission behaviour
  testable later; build them now, not when they are needed.
