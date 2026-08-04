---
id: T-31
title: Reload the TLS server identity without restarting
pillar: Transport
status: ready
priority: 6
design: docs/designs/live-endpoint-policy.md
epic: live-endpoint-policy
areas: [sipx-transport, tls, wss, security, m13, parity-wave-1]
predicate:
announcement:
note: validate then atomically swap new-handshake identity · established connections survive
---

# Reload the TLS server identity without restarting

## Goal

Rotate the certificate and key used by new TLS and WSS server handshakes without rebinding the
endpoint, interrupting established dialogs or exposing a partially updated identity.

## Acceptance

- [ ] A public endpoint operation accepts a parsed server identity and validates the complete chain
      and matching private key before it can become active.
- [ ] Publication is atomic: a concurrent-handshake test observes either the complete old pair or the
      complete new pair, never a mixture.
- [ ] Invalid replacement material returns a typed error, exposes no private bytes, and leaves the
      previous identity active for subsequent handshakes.
- [ ] Existing TLS and WSS connections and their dialogs continue on the identity with which they
      were established; reload creates no replacement tasks or sockets.
- [ ] Success and failure are observable through the endpoint's existing diagnostics. File watching,
      signals and secret-store integration remain host concerns.
- [ ] Client trust-anchor rotation, outbound mutual-TLS identity rotation and QUIC are explicitly
      unchanged unless a shared atomic primitive proves them without widening the public contract.
- [ ] Failing-first concurrent and invalid-input tests pass and `./scripts/gate.py` is green.

## Progress

- Not started. Independent M13 frontier work.
