---
id: T-2
title: Implement the UDP transport and the loopback harness
pillar: Signalling
status: backlog
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
- [ ] UDP transport binds, receives datagrams, feeds them to the core, and performs the
      core's send outputs.
- [ ] `rport` and `received` are applied per RFC 3581 when responding to a request whose
      source differs from its `Via` sent-by.
- [ ] A datagram that fails to parse is logged and dropped without disturbing the socket —
      a single malformed packet cannot stop the stack.
- [ ] Oversized datagrams are handled per the spec's limit rather than by allocation.
- [ ] `sipx-testkit` provides a loopback transport that connects two stacks in-process with
      controllable loss, duplication, reordering and delay.
- [ ] Failing-first test: `loopback_options_request_gets_200`, then the same over a real UDP
      socket on localhost.

## Progress
- Not started.

## Notes
- The fault-injection knobs on the loopback transport are what make retransmission behaviour
  testable later; build them now, not when they are needed.
