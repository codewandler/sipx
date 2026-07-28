---
id: T-12
title: Implement the QUIC transport
pillar: Signalling
status: backlog
priority: 12
design: docs/specs/sip-quic.md
epic: quic
areas: [sipx-transport]
note: track: quic · blocked by T-11
---

# Implement the QUIC transport

## Goal
SIP over QUIC per `docs/specs/sip-quic.md`, wired through resolution, the connection pool,
and the endpoint exactly as TLS and WebSocket were.

## Acceptance
- [ ] `TransportKind::Quic` in `target.rs`: `Reliable`, `default_port() == 5061`,
      `parse(b"QUIC")`; RFC 3263 resolution in `resolve.rs` admits QUIC candidates for `sips:`
      targets using the spec's NAPTR service string.
- [ ] `crates/sipx-transport/src/quic.rs`: connect/accept built on quinn, with the certificate
      policy **reused** from `tls.rs` (`ClientTls`/`ServerTls` convert into the rustls configs
      quinn consumes, ALPN `sip`) — `sip-tls.md` §3's one-implementation rule, not a second
      verifier.
- [ ] Feature `quic = ["tcp", "dep:quinn", "dep:rustls-pki-types"]`, in `default`;
      `Config::quic_client`/`quic_server`, `Handle::quic_addr()`, `sent_by_for(Quic)` using
      the listener port, and a `listen_quic()` accept loop adopting connections through the
      existing `Adopt` channel — no new `select!` branch (endpoint.rs:461).
- [ ] Pool keyed `(peer, Quic, verify_as)`; `Driver::transmit()` prefers the connection the
      request arrived on, else dials with `verify_as` as the server name; connection close
      surfaces as `tcp::Event::Closed` so `fail_transactions_on` (endpoint.rs:825) works
      unchanged.
- [ ] Error variants name the failure per `sip-tls.md` §3.1: wrong host, unknown issuer,
      wrong ALPN, connection closed.
- [ ] Failing-first tests from the spec's vector table in
      `crates/sipx-transport/tests/quic.rs` — including framing against a bare QUIC peer, not
      a second sipx (the T-8 lesson: shared framers hide framing bugs).
- [ ] The full gate passes, including `scripts/check-features.sh` with `quic` off — the T-8
      `tls`-off regression is the precedent this guards.

## Progress
- (not started)

## Notes
- quinn owns its UDP socket; the `udp` feature and its socket are untouched.
- quinn is the rustls-based QUIC stack, which keeps the existing rustls/native-certs
  certificate policy intact.
