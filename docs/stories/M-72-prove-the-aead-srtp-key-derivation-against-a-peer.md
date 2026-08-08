---
id: M-72
title: Prove the AEAD SRTP key derivation against an independent peer
pillar: Media
status: ready
priority: 22
design: docs/designs/media-security-profiles.md
epic: media-security-profiles
areas: [sipx-rtp, sipx-media, interop]
predicate:
announcement:
note: RFC 7714 publishes no KDF vector · a wrong salt placement makes two sipx endpoints interoperate with each other and nobody else, and every round-trip test still passes
---

# Prove the AEAD SRTP key derivation against an independent peer

## Goal

Get independent evidence that `M-41`'s AEAD-GCM key derivation is right. The AEAD *transform* is
pinned by RFC 7714's own published vectors; the **key derivation is not**, and no test this
repository can write will catch it being wrong.

## Acceptance

- [ ] One interop run establishes AEAD-GCM protected media with an implementation that did not
      learn its key derivation from sipx, and audio is verified as non-silent in both directions.
- [ ] Both `AEAD_AES_128_GCM` and `AEAD_AES_256_GCM` are covered, over SDES and over DTLS-SRTP,
      because the two keying paths reach the derivation differently.
- [ ] The peer, its exact revision and the negotiated profile are recorded as run evidence a
      stranger can audit, in the shape `tests/interop/` already uses.
- [ ] A failing-first negative proves the harness would actually catch a wrong derivation — for
      example a deliberately perturbed salt offset must fail the run, not merely log.
- [ ] `./scripts/gate.py` green, with the interop job registered as a gate step or in
      `NOT_RUN_LOCALLY` with a reason.

## Progress

- 2026-08-08: filed from `M-41`'s handoff, as its first stated risk. RFC 7714 publishes no KDF
  vector, so where the 96-bit master salt sits in the PRF input block rests on a reading of the
  spec rather than on a number — recorded at `docs/specs/srtp.md` §4.3 and §12.10. If that reading
  is wrong, two sipx endpoints interoperate with each other and with nobody else, and **every
  round-trip test in the tree still passes**, because both ends share the same mistake. This is the
  failure shape §12.1 already describes.

## Notes

- `tests/interop/run.sh` already supplies peer discovery by `*/profile.sh`, pinned public images,
  per-run certificates from `sipx-testkit --example issue-certs`, and a dynamic CI matrix. The
  harness is not the gap; a peer that speaks AEAD-GCM is.
- This is the same class of gap as `T-13`: an interop claim that cannot be self-proved. Unlike
  `T-13`, a third-party implementation certainly exists here — AEAD-GCM SRTP is widely deployed —
  so this one is achievable without inventing a peer.
- Until this lands, the release notes must not claim AEAD interoperation, only AEAD negotiation and
  transform correctness against the RFC's vectors.
