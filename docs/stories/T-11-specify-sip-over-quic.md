---
id: T-11
title: Specify SIP over QUIC
pillar: Signalling
status: ready
priority: 8
design: docs/designs/sip-transport.md
epic: quic
areas: [sipx-transport]
note: track: quic · a draft rather than an RFC, so it sits below the RFC work
---

# Specify SIP over QUIC

## Goal
Write `docs/specs/sip-quic.md`, in the style of `docs/specs/sip-tls.md`: normative RFC
references, the decisions no RFC makes for us stated as `[sipx]` rules, and a byte-level
test-vector table that T-12 derives its tests from. SIP-over-QUIC is not yet ratified, so the
spec is honest that the mapping choices are ours.

## Acceptance
- [ ] Normative references: RFC 9000, RFC 9001, RFC 3261 §18, RFC 3263, and the certificate
      policy of `sip-tls.md` §3 by reference (QUIC's handshake is TLS 1.3, so the TLS 1.2
      floor is moot; identity checks are identical).
- [ ] The undecided questions are decided, each as a `[sipx]` rule with rationale:
      - Via transport token (`SIP/2.0/QUIC`) and default port (5061, matching TLS: both are
        authenticated transports);
      - one SIP message per QUIC bidirectional stream, stream end as the message boundary, so
        `Content-Length` is optional — the RFC 7118 §5 reasoning `sip-tls.md` §4 already made;
      - the ALPN token (`sip`) and refusal of a peer that negotiates anything else;
      - the NAPTR service string for RFC 3263 resolution (none is registered for QUIC; the
        spec picks one and documents it);
      - 0-RTT is refused for requests: a replayed early-data request confuses transaction
        matching, and QUIC offers no mechanism to bind a SIP transaction to a handshake.
- [ ] Connection keying follows `sip-tls.md` §5: `(peer, TransportKind::Quic, verify_as)` —
      identity survives resolution.
- [ ] Response routing: QUIC connections are bidirectional, so responses return on the
      connection the request arrived on (the RFC 5923 rule, absolute as for WSS — a client
      behind NAT may have no connectable address).
- [ ] NAT and liveness: QUIC PING frames are the keepalive (no SIP-level keepalive), and
      connection migration is the QUIC stack's concern, not the transport layer's.
- [ ] A test-vector table (like the L*/W* tables) covering: certificate refusal (wrong name,
      unknown issuer), wrong-ALPN refusal, one-message-per-stream framing, response on the
      same connection, 0-RTT refusal, and connection close failing live transactions.

## Progress
- (not started)

## Notes
- `docs/specs/sip-tls.md` is the template; `docs/stories/T-6-*.md` shows how that spec story
  was scoped.
