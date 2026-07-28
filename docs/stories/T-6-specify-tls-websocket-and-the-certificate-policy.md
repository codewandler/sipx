---
id: T-6
title: Specify TLS, WebSocket and the certificate policy
pillar: Signalling
status: done
priority: 1
design: docs/designs/sip-transport.md
epic: depth
areas: [sipx-transport]
note:
---

# Specify TLS, WebSocket and the certificate policy

## Goal
Decide, in writing, what sipx verifies and what it refuses — before any handshake code exists.
The transport enum has named TLS, WS and WSS since M2 while implementing none of them, and a
`sips:` URI currently resolves to no candidate at all.

## Acceptance
- [x] `docs/specs/sip-tls.md` states which certificate checks are mandatory and which are
      configurable, and why each is on that side of the line.
- [x] States what a `sips:` URI guarantees end to end, and what it does **not** — it is
      hop-by-hop below the last proxy, and a stack that implies otherwise is lying to its user.
- [x] Decides the identity check for a SIP peer (RFC 5922): which of subjectAltName, CN and the
      SIP URI host must match, and what happens when a certificate carries several.
- [x] Decides mutual TLS: when sipx presents a client certificate, and what it does when a
      server demands one it does not have.
- [x] Decides the minimum protocol version and cipher policy, and states the consequence of
      each exclusion for real deployments.
- [x] Specifies the WebSocket handshake (RFC 7118): the subprotocol token, how a `Via` names a
      WebSocket hop, and how the connection maps to a transaction.
- [x] Records what a *failed* verification does: refuse and say which check failed, never
      downgrade, never continue with a warning.

## Progress
- Done. `docs/specs/sip-tls.md`.
- The decision that shapes the rest: **there is no "skip verification" option.** Test code that
  needs a fixture CA adds it as a trust anchor, which is a different operation with a different
  shape — it says *what* to trust rather than *that anything goes*. Every stack that ships an
  `insecure` flag eventually finds it in production, because it is exactly what a frustrated
  engineer reaches for at midnight and nothing about it is loud the next morning.
- The identity checked is the host in the URI sipx set out to reach, not the name a SRV record
  led to. Checking the resolved name lets anyone who can influence DNS choose which certificate
  is acceptable, which makes the verification decorative.
- `sips:` is documented as *transport* security, never end-to-end, because it is hop-by-hop
  below the last proxy and is routinely misread as more than that.
- Ciphers are deliberately not pinned: a hand-written list is a snapshot of one afternoon's
  opinion, and the lists people pin are why deployments still negotiate things nobody meant to
  allow.

## Notes
- The temptation in every TLS implementation is a "skip verification" flag for testing. If one
  exists it must be impossible to set by accident and loud when set.
