---
id: M-19
title: Parse and serialise the ICE attributes in SDP
pillar: Media
status: ready
priority: 7
design: docs/designs/media.md
epic: ice
areas: [sipx-sdp]
note: ice · RFC 8839 §5 · pure parsing; nothing else can negotiate until this exists
---

# Parse and serialise the ICE attributes in SDP

## Goal
Parse and serialise every attribute RFC 8839 defines, so the rest of ICE negotiates over
a typed description rather than a substring search. Pure parsing: no clock, no socket, no runtime.

## Acceptance
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

## Progress
- Not started. Cut from `M-16`'s proposed split; the Acceptance above is that proposal verbatim.

## Notes
- The spec is [`docs/specs/ice.md`](../specs/ice.md), written by `M-16` before any code. Read the
  sections its Acceptance names rather than re-deriving them from the RFCs.
- `M-16` is the tracker for this epic and stays open until every child is done.
