---
id: T-7
title: Implement the TLS transport
pillar: Signalling
status: done
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
- [x] TLS over TCP, reusing the stream framing and connection pool that already exist — a TLS
      connection differs from a TCP one in its bytes, not in its transaction handling.
- [x] Certificate verification per `T-6`, with the peer identity checked against the URI host.
- [x] A verification failure closes the connection and reports which check failed; it never
      falls back to TCP.
- [x] `sips:` resolution yields TLS candidates and the call establishes over them.
- [x] Both directions: sipx presents a certificate when acting as a server.
- [x] Failing-first test: `a_certificate_for_the_wrong_host_is_refused`, with a fixture CA so
      no test depends on a public certificate.
- [x] Interop: register over TLS against Kamailio in `tests/interop`.

## Progress
- Done. `crates/sipx-transport/src/tls.rs`, with the pool and framing from `T-3` reused rather
  than reimplemented — the connection pump is now generic over the stream, because a TLS
  connection differs from a TCP one in its bytes and in nothing else. A second copy of that
  loop would be a second place for the framing rules to drift.
- `Target` gained `verify_as`: the host from the URI, carried alongside the address. That is
  the spec's central rule made structural — the address says where to send and the name says
  who must be there, and deriving the second from the first would let whoever controls DNS
  choose which certificate is acceptable.
- Two real bugs found while wiring it up. The pool's events did not say which transport
  carried them, so a message that crossed TLS was reported as cleartext. And a TLS *server*
  could not answer at all unless it was also configured as a client — responding on the
  connection a request arrived over needs no client configuration, and requiring it left a
  pure server mute.
- The interop half is **not** done: no TLS registration against Kamailio yet. That needs a
  certificate and a Kamailio TLS configuration in `tests/interop`, and is filed as `T-10`
  rather than left implied.
