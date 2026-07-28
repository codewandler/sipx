---
id: T-7
title: Implement the TLS transport
pillar: Signalling
status: in-progress
priority: 2
design: docs/designs/sip-transport.md
epic: depth
areas: [sipx-transport]
note:
---

# Implement the TLS transport

## Goal
SIP over TLS, so `sips:` reaches something and a registration can cross an untrusted network.

## Acceptance
- [ ] TLS over TCP, reusing the stream framing and connection pool that already exist — a TLS
      connection differs from a TCP one in its bytes, not in its transaction handling.
- [ ] Certificate verification per `T-6`, with the peer identity checked against the URI host.
- [ ] A verification failure closes the connection and reports which check failed; it never
      falls back to TCP.
- [ ] `sips:` resolution yields TLS candidates and the call establishes over them.
- [ ] Both directions: sipx presents a certificate when acting as a server.
- [ ] Failing-first test: `a_certificate_for_the_wrong_host_is_refused`, with a fixture CA so
      no test depends on a public certificate.
- [ ] Interop: register over TLS against Kamailio in `tests/interop`.

## Progress
- Not started.
