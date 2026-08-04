---
id: M-46
title: Multiplex RTCP and negotiate the DTLS setup role
pillar: Media
status: backlog
priority: 6
design: docs/designs/demand.md
epic: demand
areas: [sipx-sdp, sipx-media, sipx-rtp, beta4]
predicate:
announcement: [3, 4]
note: RFC 5761 + RFC 4145 · both are hard blockers for browser interop · prerequisites for M-38
---

# Multiplex RTCP and negotiate the DTLS setup role

## Goal

Support `a=rtcp-mux` and `a=setup:actpass` role negotiation, removing the two hard blockers between
sipx and any browser-side peer.

## Acceptance

- [ ] `a=rtcp-mux` (RFC 5761) is offered, answered and honoured: when multiplexing is agreed, RTP
      and RTCP share one port and the demultiplexer routes by payload type per RFC 5761 §4. A
      failing-first test asserts RTCP arriving on the RTP port is processed, not dropped.
- [ ] When the peer does not agree, sipx falls back to separate ports without renegotiating, and a
      test covers the non-mux path so the fallback cannot rot.
- [ ] `a=setup` (RFC 4145, as profiled by RFC 5763 §5) is negotiated properly: sipx offers
      `actpass`, honours an answer of `active` or `passive`, and takes the complementary DTLS role.
      A failing-first test covers both answers.
- [ ] An answer selecting a role sipx cannot take is refused with a typed error rather than
      proceeding into a handshake that cannot complete.
- [ ] Interaction with the existing DTLS-SRTP path is explicit: fingerprint verification still
      completes before any exported key reaches a media session, and a mismatch still yields no keys.
      This story must not weaken `M-28`'s guarantee — a test asserts it still holds.
- [ ] `docs/rfc/registry.toml` gains RFC 5761 and updates the RFC 4145 and 5763 rows in the same
      commit; `rfc-report.py --check` green.
- [ ] `./scripts/gate.py` green.

## Progress
- (not started)

## Notes
- Both were reported as concrete blockers against a comparable stack's DTLS-SRTP path. They are
  small relative to their consequence: without either, a browser peer cannot establish media at all.
- Sequence these **before** `M-38` (browser-compatible WebRTC audio) — that story assumes both.
- Pairs naturally with `M-41` (AEAD SRTP profiles), which browser peers also expect.
