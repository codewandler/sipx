---
id: P-13
title: Prove the complete diagnostic phone from a shell
pillar: Phone
status: backlog
priority: 11
design: docs/designs/phone.md
epic: phone
areas: [sipx-cli, interop, docs]
note: blocked by P-8 through P-12 and the call-layer blockers named by them
---

# Prove the complete diagnostic phone from a shell

## Goal

Make the phone epic's exit criterion one reproducible matrix rather than a collection of lower-layer
claims.

## Acceptance

- [ ] One bounded runner executes `DPH-1` … `DPH-12` and emits a checked matrix with requested and
      negotiated paths.
- [ ] Real-network cases cover all five signalling transports, G.711 and Opus, plain RTP, SDES,
      DTLS-SRTP, early media, authenticated INVITE and an ICE NAT case.
- [ ] Two independently implemented peers cover every signalling transport the public README claims.
- [ ] Device evidence uses a virtual loopback on Linux; no test requires a human or a fixed sleep.
- [ ] The public CLI reference is generated/checked against `--help` and the JSON schema.
- [ ] The full gate is green with default, no-default and all feature sets.

## Progress

- Not started.
