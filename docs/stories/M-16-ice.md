---
id: M-16
title: Implement ICE
pillar: Media
status: blocked
priority:
design: docs/designs/media.md
epic: ice
areas: [sipx-media, sipx-sdp, sipx-rtp]
note: epic tracker · split into M-19 … M-24 · spec is docs/specs/ice.md, written first
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
  TURN client, RFC 8656, a protocol of its own. The proposed split is below; the seams are the
  spec's own section boundaries, and each child has a failing-first test that does not depend on a
  later child. IDs are left unallocated because allocating one means regenerating the board.
- **Open decision blocking the STUN codec** (spec §15): `MESSAGE-INTEGRITY` is HMAC-SHA1, and
  neither `sipx-media` nor `sipx-sdp` lists `hmac`/`sha1` — `sipx-rtp` does, for SRTP. Either the
  codec's crate gains two dependency lines that already exist in the workspace, or the codec goes
  where they already are. A crate-graph decision, not an implementation detail.
- The registry was deliberately **not** touched: with no code, moving RFC 8445 off `none` would be
  a claim `rfc-report.py --check` is right to want evidence for, and the table is read as a
  measurement.

## Proposed split
> **Cut.** These six are now stories `M-19` … `M-24`, with this section's Acceptance carried
> across verbatim. They are the contract; what follows is kept as the record of why the split
> falls where it does.

Six children, in dependency order. Each is `pillar: Media`, `epic: conformance`, `design:
docs/designs/media.md`, and each cites [`docs/specs/ice.md`](../specs/ice.md) as its spec. The
Acceptance below is the contract; the spec section named beside each item is where the detail is.

### 1 — The ICE attributes in SDP (RFC 8839 §5) · `areas: [sipx-sdp]`
**Goal.** Parse and serialise every attribute RFC 8839 defines, so the rest of ICE negotiates over
a typed description rather than a substring search. Pure parsing: no clock, no socket, no runtime.

- [ ] `candidate` (§5.1) parses and serialises: the RFC's own example line round-trips byte-identically,
      including `raddr`/`rport`, and unknown `cand-extension` pairs survive rather than being rejected.
- [ ] `ice-ufrag`/`ice-pwd` (§5.4) at session and media level with **media level winning**; ≤32 chars
      on send and ≤256 accepted on receive.
- [ ] `ice-options` (§5.6), `ice-lite` and `ice-mismatch` (§5.3) at their stated levels,
      `remote-candidates` (§5.2), `ice-pacing` (§5.5).
- [ ] `priority` is range-checked to 1..=2^31−1 on parse. The grammar is `1*10DIGIT`, so `4294967295`
      parses, and the §6.1.2.3 pair-priority arithmetic overflows `u64` on it — the check is what
      makes that arithmetic safe (spec §4, §6.2).
- [ ] A `candidate` line with an FQDN or an unsupported address family is **ignored** and the rest of
      the description survives; a transport other than UDP is accepted and discarded (spec §3).
- [ ] Failing-first test: `the_rfc_8839_candidate_example_round_trips_unchanged`.
- [ ] No new dependency, and `sipx-sdp` gains no runtime, socket or clock read.

### 2 — STUN for connectivity checks · `areas: [sipx-media]` (see the crate-graph decision)
**Goal.** A STUN codec that can both send and answer an ICE connectivity check — the attributes, the
credentials and the two integrity values — so the agent has a transaction to run.

- [ ] RFC 5769 §2.1's sample request is produced **byte-for-byte** from its stated username, password,
      `PRIORITY` and `ICE-CONTROLLED` tiebreaker. The encoder, not the decoder: the tag was computed
      by the IETF, so this is the direction that cannot be self-confirming.
- [ ] `MESSAGE-INTEGRITY` (RFC 5389 §15.4) over the length-adjusted message, then `FINGERPRINT`
      (§15.5) as CRC-32 XOR `0x5354554e` computed last, in that order; the received tag is compared
      constant-time (spec §11.2).
- [ ] Username direction: `<peer-ufrag>:<our-ufrag>` outbound keyed with the **peer's** password,
      `<our-ufrag>:<peer-ufrag>` inbound keyed with ours. Reversed, the agent answers nothing and its
      own checks are all rejected, and it looks exactly like a network fault.
- [ ] `PRIORITY`, `USE-CANDIDATE` (zero-length flag), `ICE-CONTROLLED`/`ICE-CONTROLLING`,
      `ERROR-CODE` 487 and `XOR-MAPPED-ADDRESS` encode as well as decode (spec §11.1).
- [ ] A §11 keepalive is a Binding **Indication** with `FINGERPRINT`, no credential and nothing else.
- [ ] No panic, no raw index and no wrapping length arithmetic on any byte string: this parser eats
      unauthenticated datagrams from anyone who can reach the media port.
- [ ] `sipx_transport::stun` is reused where it fits and gains nothing — it declares in its own header
      that ICE needs a different module, not more attributes bolted on.
- [ ] **This story makes the crate-graph decision** (spec §15): `MESSAGE-INTEGRITY` is HMAC-SHA1, and
      neither `sipx-media` nor `sipx-sdp` lists `hmac`/`sha1` while `sipx-rtp` does. Either the
      codec's crate gains two dependency lines, or the codec goes where they already are.
- [ ] Failing-first test: `a_connectivity_check_encodes_to_the_rfc_5769_sample_request`.

### 3 — The ICE agent: candidates, checklists, checks, nomination · `areas: [sipx-media]` · after 2
**Goal.** The sans-IO state machine — gather, prioritise, pair, order, check, resolve role conflict,
nominate — as a pure function of events, so the socket work is a driver over it.

- [ ] The §5.1.2.1 formula exactly, asserted against the spec's three-row table, including the
      `1694498815` RFC 8839 prints in its own example.
- [ ] `PRIORITY` in a check uses the **peer-reflexive** type preference (§7.1.1), not the candidate's
      own — otherwise the peer prioritises the peer-reflexive candidate it learns differently from us.
- [ ] Foundations per §5.1.1.3; pairing per §6.1.2.2 including the link-local rule; pair priority per
      §6.1.2.3; pruning per §6.1.2.4; the configurable 100-pair limit per §6.1.2.5.
- [ ] Initial pair states per §6.1.2.6, asserted against the RFC's own three-checklist,
      five-foundation worked example.
- [ ] Role conflict per §7.3.1.1: all seven rows of the spec's §7.3 table including the `T = V` row,
      and on receiving a 487 the agent switches role, **changes its tiebreaker** (§7.2.5.1),
      recomputes every pair priority and re-runs the check as a triggered one.
- [ ] **Regular nomination only** (§8.1.1). Aggressive nomination is absent and there is no option to
      enable it; the controlled side still tolerates a peer that nominates twice by selecting the
      highest-priority nominated pair.
- [ ] Peer-reflexive candidates learned in both directions (§7.2.5.3.1, §7.3.1.3); triggered checks
      (§7.3.1.4) preempt the checklist; a non-symmetric response fails the pair (§7.2.5.2.1).
- [ ] Ta, RTO, Rc and Rm per §14, configurable, no literals in the machine, RTO recomputed per
      transaction because it depends on how many checks are outstanding.
- [ ] Sans-IO: no `tokio`, no clock read, no socket. Time arrives as `TimerFired`.
- [ ] Failing-first test: `two_agents_that_both_start_controlling_converge_on_one_role`.

### 4 — Drive ICE on the media port · `areas: [sipx-media]` · after 1 and 3
**Goal.** Bind the agent to the socket the media already uses, so a nominated pair carries audio —
and so a peer that offers no ICE still gets symmetric RTP exactly as today.

- [ ] Host candidates from the bound `MediaPort`; server-reflexive over `sipx_transport::stun`
      against a configured STUN server; component 2 only when the control port was actually obtained.
- [ ] Checks demultiplexed by `dtls::classify` (RFC 5764 §5.1.2) on the port media uses: a check never
      reaches the jitter buffer and an RTP packet never reaches the agent.
- [ ] Keepalives per §11: Binding Indication, Tr = 15 s, only on selected pairs.
- [ ] The selected pair replaces symmetric-RTP address learning for a stream that has one; a stream
      that has none keeps it.
- [ ] **A peer offering no `a=candidate` gets exactly today's behaviour** — nothing offered, no checks,
      no timers, symmetric RTP. The existing media suite passing unchanged is the regression proof.
      A stack that requires ICE to place a call has regressed.
- [ ] `ice-mismatch` (§5.3) reported in the answer when the offer's default destination for a
      component had no matching candidate, and RFC 3264 procedures used for that stream instead.
- [ ] The RFC registry gains RFC 8839 and moves RFC 8445 off `none`, with `docs/compliance.md`
      regenerated by `./scripts/rfc-report.py` in the same change.
- [ ] Failing-first test: `a_nominated_candidate_pair_carries_audio_when_the_host_candidates_cannot`
      — the test `M-16` names, and the reason this child comes after the agent rather than beside it.

### 5 — Recognise and act on an ICE restart · `areas: [sipx-media, sipx-call]` · after 4
**Goal.** Survive a mid-call address change: a re-offer whose `ice-ufrag` and `ice-pwd` both changed
starts a new ICE session while the old pair keeps carrying audio.

- [ ] **Both** `ice-ufrag` and `ice-pwd` changed is a restart (RFC 8839 §4.4.1.1.1); one alone is not,
      and the same value moving between session and media level is explicitly not.
- [ ] A restart regenerates the tiebreaker, re-gathers, rebuilds the checklists, and may redetermine
      the role.
- [ ] Media keeps flowing on the previously selected pair until the new session selects one. A
      restart that goes silent is worse than no restart.
- [ ] `c=0.0.0.0` is not used for hold; hold stays `a=inactive`/`a=sendonly` (RFC 3264).
- [ ] Failing-first test: `a_reoffer_that_changes_both_ufrag_and_pwd_restarts_ice_without_dropping_audio`.

### 6 — Gather a relayed candidate from a configured relay · `areas: [sipx-media]` · after 4
**Goal.** The last resort ICE keeps for when neither host nor reflexive candidates reach: allocate on
a configured TURN relay and offer the relayed candidate. Running a relay stays out of scope.

- [ ] RFC 8656 Allocate, Refresh, CreatePermission and Send/Data against a configured relay with
      long-term credentials. **This is a third RFC, and it is why `M-16` could not be one story.**
- [ ] The relayed candidate's type preference is 0, and its `raddr`/`rport` are the mapped address
      from the Allocate response (RFC 8839 §5.1).
- [ ] Allocations kept alive until ICE completes (§5.1.1.4).
- [ ] A relay that is unreachable or refuses degrades to the other candidate types rather than
      failing the call.
- [ ] Failing-first test: `a_relayed_candidate_is_offered_when_a_relay_is_configured`.

## Notes
- This is the largest media story since `M-14`, and for the same reason: it is a protocol inside the
  media path with its own state machine. The state machine belongs in a sans-IO module the way the
  transaction machines do — candidate pairs, check state and nomination are a function of events, and
  the socket work is a driver over it.
- A relayed candidate needs a relay, and standing one up is not this story. Gathering and using a
  relayed candidate when one is configured is; running a relay is not.
- Ordering against `M-15`: DTLS-SRTP keys the stream, ICE finds the path for it. They meet at the
  same socket, so whichever lands second inherits the integration. `M-15` is in M6 and lands first.
