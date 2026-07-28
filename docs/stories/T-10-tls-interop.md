---
id: T-10
title: Verify TLS against a real server
pillar: Signalling
status: ready
priority: 5
design: docs/designs/sip-transport.md
epic: depth
areas: [sipx-transport]
note: gap left explicitly by T-7
---

# Verify TLS against a real server

## Goal
Register over TLS against Kamailio, so the handshake is verified against an implementation that
did not learn it from sipx.

## Acceptance
- [ ] `tests/interop` generates a certificate and configures Kamailio to serve TLS with it.
- [ ] sipx registers over TLS and the interop suite asserts it.
- [ ] The negative is asserted too: sipx refuses a Kamailio presenting a certificate for the
      wrong name, rather than connecting anyway.

## Progress
- Not started. `T-7` implemented and tested TLS against fixture certificates; this is the half
  that proves another implementation agrees.
