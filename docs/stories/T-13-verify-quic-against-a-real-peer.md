---
id: T-13
title: Verify QUIC against a real peer
pillar: Signalling
status: backlog
priority: 13
design: docs/specs/sip-quic.md
epic: quic
areas: [sipx-transport]
note: track: quic · T-12 delivered the transport; independent-peer evidence remains
---

# Verify QUIC against a real peer

## Goal
Prove the handshake and framing against an implementation that did not learn them from sipx,
as T-10 did for TLS.

## Acceptance
- [ ] `tests/interop` exchanges a REGISTER over QUIC with a non-sipx peer. If no third-party
      SIP-over-QUIC server exists by the time this is picked up, the fallback is a harness
      that speaks the spec's mapping (ALPN, one message per stream) over an independent QUIC
      stack — never a second sipx endpoint, which would share the framer.
- [ ] The negatives are asserted too: a certificate for the wrong name and a wrong ALPN are
      refused **immediately**, not by timeout (the T-10 lesson — a timeout lets a hung stack
      pass as a refusal).
- [ ] Each negative is confirmed non-vacuous by handing it the valid input and watching it
      fail.

## Progress
- Not started. T-12 delivered the transport and its bare-peer vectors; this story owns evidence
  from an independently implemented peer.

## Notes
- Reuse the T-10 fixture authority (`cargo run -p sipx-testkit --example issue-certs`) so the
  QUIC interop tests trust the same CA as the TLS ones.
