---
id: T-13
title: Verify QUIC against a real peer
pillar: Signalling
status: backlog
priority:
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

- 2026-08-08: **readiness audit — the story cannot be satisfied as written without a scope decision.**
  `tests/interop/run.sh` already supplies peer discovery, pinned images, per-run certificates and a
  dynamic CI matrix, so the harness is not the gap. The gap is that **no third-party SIP-over-QUIC
  server exists** — there is no RFC for it — and `quinn` is the only QUIC stack in the lockfile, so
  `T-12`'s existing bare peer is quinn+rustls too and is *not* independent in the sense `T-10`
  established for TLS. Satisfying this needs (a) a peer built on a different stack — `aioquic`
  (Python) or `quic-go` (Go); `neqo` is MPL-2.0 and fails `deny.toml` — and (b) the repository's
  first Dockerfile, because no public image speaks `sip/2`. Deferred out of the rc.4 wave until
  that admission decision is taken.

## Notes
- Reuse the T-10 fixture authority (`cargo run -p sipx-testkit --example issue-certs`) so the
  QUIC interop tests trust the same CA as the TLS ones.
