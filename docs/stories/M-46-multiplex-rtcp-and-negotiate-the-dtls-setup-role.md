---
id: M-46
title: Multiplex RTCP and negotiate the DTLS setup role
pillar: Media
status: done
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

- [x] `a=rtcp-mux` (RFC 5761) is offered, answered and honoured: when multiplexing is agreed, RTP
      and RTCP share one port and the demultiplexer routes by payload type per RFC 5761 §4. A
      failing-first test asserts RTCP arriving on the RTP port is processed, not dropped.
- [x] When the peer does not agree, sipx falls back to separate ports without renegotiating, and a
      test covers the non-mux path so the fallback cannot rot.
- [x] `a=setup` (RFC 4145, as profiled by RFC 5763 §5) is negotiated properly: sipx offers
      `actpass`, honours an answer of `active` or `passive`, and takes the complementary DTLS role.
      A failing-first test covers both answers.
- [x] An answer selecting a role sipx cannot take is refused with a typed error rather than
      proceeding into a handshake that cannot complete.
- [x] Interaction with the existing DTLS-SRTP path is explicit: fingerprint verification still
      completes before any exported key reaches a media session, and a mismatch still yields no keys.
      This story must not weaken `M-28`'s guarantee — a test asserts it still holds.
- [x] `docs/rfc/registry.toml` gains RFC 5761 and updates the RFC 4145 and 5763 rows in the same
      commit; `rfc-report.py --check` green.
- [x] `./scripts/gate.py` green.

## Progress

- In progress. `docs/specs/rtcp-mux-setup.md` owns the normative composition contract.
- SDP now negotiates mux and complementary, capability-checked DTLS setup roles with typed
  refusals. Initial calls offer mux; in-dialog descriptions preserve the running socket mode.
- Media uses one receive owner for muxed RTP/RTCP, retains the separate control-port path as the
  fallback, refuses colliding payload types, and authenticates SRTCP before parsing it.
- ICE gathering now follows the negotiated shape: initial offers retain component 2 plus an
  explicit `a=rtcp` fallback, while a mux answer emits and checks component 1 alone. Live muxed
  RTCP is asserted to leave from the RTP socket address.
- In-dialog offers and answers cannot silently change the running socket mode; a typed refusal
  leaves ICE and media state untouched. Session-level setup roles use the same resolver in SDP and
  the handshake, and `holdconn` is refused before answer-side binding or gathering.
- Failing-first SDP vectors and live socket tests cover mux, separate fallback, both legal DTLS
  answers, unsupported roles, and the existing fingerprint-mismatch/no-keys guarantee.
- The command-level server-reflexive ICE vector keeps both initial-offer components and its
  explicit RTCP fallback while offering mux, then proves the selected component-1 path carries
  audio. Its wildcard CLI bind regression is pinned by the implicit media-address selection test.
- RFC 5761 is registered and the RFC 4145/5763 claims are updated; the generated 73-RFC report and
  its consistency check are current.

## Notes
- Both were reported as concrete blockers against a comparable stack's DTLS-SRTP path. They are
  small relative to their consequence: without either, a browser peer cannot establish media at all.
- Sequence these **before** `M-38` (browser-compatible WebRTC audio) — that story assumes both.
- Pairs naturally with `M-41` (AEAD SRTP profiles), which browser peers also expect.
