---
id: T-11
title: Specify SIP over QUIC
pillar: Signalling
status: done
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
- [x] Normative references: RFC 9000, RFC 9001, RFC 3261 §18, RFC 3263, and the certificate
      policy of `sip-tls.md` §3 by reference (QUIC's handshake is TLS 1.3, so the TLS 1.2
      floor is moot; identity checks are identical).
- [x] The undecided questions are decided, each as a `[sipx]` rule with rationale:
      - Via transport token (`SIP/2.0/QUIC`) and default port (5061, matching TLS: both are
        authenticated transports);
      - one SIP message per QUIC bidirectional stream, stream end as the message boundary, so
        `Content-Length` is optional — the RFC 7118 §5 reasoning `sip-tls.md` §4 already made;
      - the ALPN token and refusal of a peer that negotiates anything else — the token is
        `sip/2`, **not** `sip` as this story said; see Progress;
      - the NAPTR service string for RFC 3263 resolution (none is registered for QUIC; the
        spec picks one and documents it);
      - 0-RTT is refused for requests: a replayed early-data request confuses transaction
        matching, and QUIC offers no mechanism to bind a SIP transaction to a handshake.
- [x] Connection keying follows `sip-tls.md` §5: `(peer, TransportKind::Quic, verify_as)` —
      identity survives resolution.
- [x] Response routing: QUIC connections are bidirectional, so responses return on the
      connection the request arrived on (the RFC 5923 rule, absolute as for WSS — a client
      behind NAT may have no connectable address).
- [x] NAT and liveness: QUIC PING frames are the keepalive (no SIP-level keepalive), and
      connection migration is the QUIC stack's concern, not the transport layer's.
- [x] A test-vector table (like the L*/W* tables) covering: certificate refusal (wrong name,
      unknown issuer), wrong-ALPN refusal, one-message-per-stream framing, response on the
      same connection, 0-RTT refusal, and connection close failing live transactions.

## Progress
- Done: [`docs/specs/sip-quic.md`](../specs/sip-quic.md), in the shape of `sip-tls.md`, with a
  `Q1`–`Q24` vector table for `T-12` to derive tests from.
- **The story named the wrong ALPN token.** It said `sip`; the IANA ALPN registry has **`sip/2`**
  for SIP, registered under RFC 3261. `sip` is the *WebSocket subprotocol* from RFC 7118 §4 — a
  different registry with a different value. The spec uses `sip/2`, and `Q5` asserts that a peer
  negotiating `sip` is refused, because the two are easy enough to confuse that the confusion is
  worth a test.
- The NAPTR service is `SIPS+D2Q`. Nothing is registered for QUIC; the existing tags are
  `SIP+D2U`/`D2T`/`D2S`/`D2W` and `SIPS+D2T`/`D2W`/`D2S`, so `D2Q` follows the pattern. **Only
  the `SIPS` form**, because RFC 9001 leaves no unencrypted QUIC — a `SIP+D2Q` would advertise an
  authenticated transport under the scheme that promises nothing.
- Two rules are there because QUIC differs from every transport sipx already has:
  - **The pool key must not include the observed peer address.** Connections survive an address
    change by design (RFC 9000 §9); keying on the address would drop a connection every time a
    phone moved from Wi-Fi to cellular, which is exactly what migration exists to prevent.
  - **A non-QUIC datagram on the QUIC port is dropped silently.** Answering it, even with an
    error, turns 5061/udp into a reflector.
- 0-RTT is refused in both directions. RFC 9001 §9.2 says early data is replayable and QUIC
  gives no way to bind a SIP transaction to a handshake; a replayed request whose transaction has
  already terminated becomes a second call. The saving is one round trip on a transport whose
  handshake is already cheap.
- The spec opens by saying plainly that no RFC defines this mapping and that the transport is
  not interoperable with anything else except by coincidence — and §7 records that QUIC is never
  chosen implicitly, because an unratified mapping has to be opted into.

## Notes
- `docs/specs/sip-tls.md` is the template; `docs/stories/T-6-*.md` shows how that spec story
  was scoped.
