---
id: M-16
title: Implement ICE
pillar: Media
status: in-progress
priority: 7
design: docs/designs/media.md
epic: conformance
areas: [sipx-media, sipx-sdp, sipx-rtp]
note: M10 · RFC 8445 + 8839 · the NAT cases symmetric RTP does not solve
---

# Implement ICE

## Goal
Establish a media path where symmetric RTP cannot: candidate gathering, connectivity checks and
nomination, so two endpoints behind NATs that never see each other's real addresses still exchange
audio.

## Acceptance
- [ ] Candidates are gathered and prioritized per RFC 8445: host candidates (§5.1.1.1),
      server-reflexive and relayed candidates (§5.1.1.2), foundations (§5.1.1.3), and the priority
      formula of §5.1.2.1 — the formula, not an approximation of it, because the priority ordering is
      what makes two independent implementations agree on which pair wins.
- [ ] Checklists are formed and ordered per §6.1.2.2 and §6.1.2.3, and connectivity checks run as
      STUN transactions over the same port the media will use.
- [ ] **Regular nomination only** (§8.1.1). Aggressive nomination "has been deprecated in this
      specification" (§4) and must not be implemented, not even as an option.
- [ ] Role conflict is resolved per §7.3.1.1 with `ICE-CONTROLLING`/`ICE-CONTROLLED` and the tiebreak
      value, rather than by assuming the offerer controls. Two endpoints that both think they control
      never converge, and it is the failure mode that only appears when both ends run the same stack.
- [ ] Keepalives per §11, so a nominated pair does not lapse mid-call behind a NAT with a short
      binding lifetime — the same failure `T-15` fixes for signalling.
- [ ] The SDP side is RFC 8839, not invented: `candidate` (§5.1), `ice-ufrag` and `ice-pwd` (§5.4),
      `ice-options` (§5.6), `ice-lite` and `ice-mismatch` (§5.3), `remote-candidates` (§5.2), with the
      initial offer and answer per §4.3.1 and §4.3.2 and `ice2` in `ice-options`. RFC 8445 §3 says the
      encoding is out of scope for itself: "The specific details […] for different protocols using
      ICE […] are described in separate usage documents."
- [ ] An ICE restart (RFC 8839 §4.4.1.1.1 — both `ice-ufrag` and `ice-pwd` change) is recognised and
      acted on, because a mid-call address change is the case ICE exists to survive.
- [ ] Interworking with a peer that offers no ICE at all falls back to what sipx does today —
      symmetric RTP — with no degradation for the common case. A stack that requires ICE to place a
      call has regressed.
- [ ] ICE-lite (§5.2, §6.2, §8.2) is either implemented or explicitly deferred with a reason, and the
      choice is recorded rather than left to a reader of the code.
- [ ] Trickle ICE is out of scope and the story says so: it is a separate document and a separate
      offer/answer model, and nothing has asked for it.
- [ ] The RFC registry gains a row for RFC 8839 and moves RFC 8445 off "not started" in the same
      change.
- [ ] Failing-first test: `a_nominated_candidate_pair_carries_audio_when_the_host_candidates_cannot`.

## Progress
- **Spec written: [`docs/specs/ice.md`](../specs/ice.md).** Normative references, the sans-IO
  input/output contract, the §5.1.2.1 formula with a worked table, foundations, checklist formation
  and pruning, the pair-state table, the §7.3.1.1 role-conflict table, regular nomination with a
  stated stopping criterion, the timer table, the STUN attribute profile and credential direction,
  the RFC 8839 grammar, offer/answer/restart, the no-ICE fallback, and eight test vectors. No code.
- **ICE-lite: deferred**, reason recorded in the spec §12 — the lite role is for an agent already on
  a public address that never gathers or checks, which is the opposite of sipx's deployment, and it
  is an endpoint-wide property so supporting it means a second nomination path alive in the same
  binary. *Interoperating with a lite peer is not deferred*: `a=ice-lite` in a remote description
  makes sipx controlling unconditionally (§6.1.1), and that has its own test.
- **Trickle ICE: out of scope**, stated in the spec's out-of-scope list with the reason (a separate
  document and a separate offer/answer model; half of it is worse than none).
- **This story is too large to land in one pass and should be split.** Twelve Acceptance items over
  two RFCs, and Acceptance item 1 quietly contains a third: "relayed candidates (§5.1.1.2)" is a
  TURN client, RFC 8656, a protocol of its own. The proposed split is M-16a … M-16f in the handoff;
  the seams are the spec's own section boundaries, and each child has a failing-first test that does
  not depend on a later child.
- **Open decision blocking the STUN codec** (spec §15): `MESSAGE-INTEGRITY` is HMAC-SHA1, and
  neither `sipx-media` nor `sipx-sdp` lists `hmac`/`sha1` — `sipx-rtp` does, for SRTP. Either the
  codec's crate gains two dependency lines that already exist in the workspace, or the codec goes
  where they already are. A crate-graph decision, not an implementation detail.
- The registry was deliberately **not** touched: with no code, moving RFC 8445 off `none` would be
  a claim `rfc-report.py --check` is right to want evidence for, and the table is read as a
  measurement.

## Notes
- This is the largest media story since `M-14`, and for the same reason: it is a protocol inside the
  media path with its own state machine. The state machine belongs in a sans-IO module the way the
  transaction machines do — candidate pairs, check state and nomination are a function of events, and
  the socket work is a driver over it.
- A relayed candidate needs a relay, and standing one up is not this story. Gathering and using a
  relayed candidate when one is configured is; running a relay is not.
- Ordering against `M-15`: DTLS-SRTP keys the stream, ICE finds the path for it. They meet at the
  same socket, so whichever lands second inherits the integration. `M-15` is in M6 and lands first.
