---
id: T-12
title: Implement the QUIC transport
pillar: Signalling
status: done
priority: 8
design: docs/specs/sip-quic.md
epic: quic
areas: [sipx-transport]
note: track: quic · T-11 supplied the mapping; T-13 owns independent-peer evidence
---

# Implement the QUIC transport

## Goal
SIP over QUIC per `docs/specs/sip-quic.md`, wired through resolution, the connection pool,
and the endpoint exactly as TLS and WebSocket were.

## Acceptance
- [x] `TransportKind::Quic` in `target.rs`: `Reliable`, `default_port() == 5061`,
      `parse(b"QUIC")`; RFC 3263 resolution in `resolve.rs` admits QUIC candidates for `sips:`
      targets using the spec's NAPTR service string.
- [x] `crates/sipx-transport/src/quic.rs`: connect/accept built on quinn, with the certificate
      policy **reused** from `tls.rs` (`ClientTls`/`ServerTls` convert into the rustls configs
      quinn consumes, ALPN `sip/2`) — `sip-tls.md` §3's one-implementation rule, not a second
      verifier.
- [x] Feature `quic = ["tcp", "tls", "dep:quinn", "dep:rustls-pki-types"]`, in `default`;
      `Config::quic_client`/`quic_server`, `Handle::quic_addr()`, `sent_by_for(Quic)` using
      the listener port, and a `listen_quic()` accept loop adopting connections through the
      existing `Adopt` channel — no new `select!` branch (endpoint.rs:461).
- [x] Pool keyed `(peer, Quic, verify_as)` with no WebSocket path; a response uses the exact
      bidirectional stream its request arrived on, while an outbound request dials with
      `verify_as` as the server name. A close first surfaces as cause-preserving
      `tcp::Event::QuicClosed`; the tracked-task wrapper's subsequent `Closed` is stale and has
      no second effect.
- [x] Error variants name the failure per `sip-tls.md` §3.1: wrong host, unknown issuer,
      wrong ALPN, connection closed.
- [x] Failing-first tests from the spec's vector table in
      `crates/sipx-transport/tests/quic.rs` and the TLS configuration tests — including framing
      against a bare QUIC peer, not a second sipx (the T-8 lesson: shared framers hide framing
      bugs).
- [x] The full gate passes, including `scripts/check-features.sh` with `quic` off — the T-8
      `tls`-off regression is the precedent this guards.

## Progress
- Implemented QUIC configuration, endpoint adoption, pooled connections, resolution, stream
  framing and same-stream responses.
- Q1–Q24 have executable coverage, including a resumed-session Q13 rejection, socket-rebind Q17
  migration, cause-bearing Q14 close, and paused-time Q20 observation of a QUIC PING with no SIP
  request.
- `scripts/check-features.sh` covers the minimal QUIC-on combination and passes with QUIC both on
  and off. The full 25-step repository gate passed on the combined tree before closure.

## Notes
- quinn owns its UDP socket; the `udp` feature and its socket are untouched.
- quinn is the rustls-based QUIC stack, which keeps the existing rustls/native-certs
  certificate policy intact.
