---
id: M-47
title: Reject replayed SRTCP with a separate replay window
pillar: Media
status: ready
priority: 4
design: docs/specs/srtp.md
epic: media-security-profiles
areas: [sipx-rtp, sipx-media, security, beta4]
predicate: 4
announcement: 2
note: known gap in srtp.md §12.2 · authenticate first, then reject a repeated SRTCP index without touching the SRTP window
---

# Reject replayed SRTCP with a separate replay window

## Goal

Close the known replay gap in the shipped SRTCP receiver before browser audio puts multiplexed
control traffic on the same exposed component as media.

## Acceptance

- [ ] A failing-first test proves that one authenticated SRTCP packet is accepted once and rejected
      when replayed, while a distinct packet with the next index remains acceptable.
- [ ] SRTCP owns a replay list separate from SRTP, keyed by the explicit 31-bit SRTCP index as RFC
      3711 §3.4 requires. Advancing either window cannot reject a valid packet in the other.
- [ ] Authentication completes before decryption and before either replay window changes. A forged
      high-index packet cannot move the window or make a later authentic packet look old.
- [ ] Wrap and too-old behavior are specified and tested at the boundary of the held window without
      a wall-clock wait or an unbounded loop.
- [ ] The mutation proof in Progress records that removing the SRTCP replay check makes the named
      replay test fail; round-trip success alone is not evidence.
- [ ] `docs/specs/srtp.md` §12.2 moves from open to fixed with the implementation and test named,
      and the RFC 3711 registry evidence is updated in the same commit.
- [ ] `./scripts/gate.py` green.

## Progress

- Filed from the ownerless known gap already recorded in `docs/specs/srtp.md` §12.2. Because a
  replayed authenticated control packet is a known-wrong shipped path, this story bears on alpha
  predicate 4 until it is closed.

## Notes

- This precedes `M-50`: RTCP multiplexing and one-component browser media increase the reachability
  of SRTCP, but do not change the replay rule.
- This is not `M-41`'s AEAD-profile work. The replay invariant applies to the protection profile
  already shipped and must not wait for another cipher suite.
