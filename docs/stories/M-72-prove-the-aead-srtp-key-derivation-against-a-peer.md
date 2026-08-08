---
id: M-72
title: Prove the AEAD SRTP key derivation against an independent peer
pillar: Media
status: in-progress
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

- [x] One interop run establishes AEAD-GCM protected media with an implementation that did not
      learn its key derivation from sipx, and audio is verified as non-silent in both directions.
- [ ] Both `AEAD_AES_128_GCM` and `AEAD_AES_256_GCM` are covered, over SDES and over DTLS-SRTP,
      because the two keying paths reach the derivation differently.
- [x] The peer, its exact revision and the negotiated profile are recorded as run evidence a
      stranger can audit, in the shape `tests/interop/` already uses.
- [x] A failing-first negative proves the harness would actually catch a wrong derivation — for
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

- 2026-08-08: **the derivation is right for `AEAD_AES_256_GCM`, proved against a native browser
  over DTLS-SRTP.** The peer that was missing turned out to be already in the tree: the `M-51`
  harness in `tests/browser-audio/` runs a real browser, and in the `browser-answerer` role sipx is
  the DTLS server and selects its strongest profile. That run negotiated `AEAD_AES_256_GCM` and
  carried non-silent Opus both ways — the browser derived its own session keys from the RFC 5764
  exporter block by its own reading of RFC 7714 §11, so agreement is not something sipx can arrange
  with itself. `driver.py:251` had recorded the profile since `M-51` and asserted only that it was
  non-empty, so the evidence existed and the claim did not.

  What changed: the profile is checked against the registry rather than for non-emptiness, at least
  one role must have keyed with AEAD-GCM or the run is refused, and the peer's exact revision is
  captured from the WebDriver session's negotiated capabilities — the browser's own answer, not the
  page's claim about itself. The run now emits `aead_key_derivation` naming the witness role, the
  profile and the peer revision.

  The negative is measured, not asserted. Right-aligning the master salt against octet 14 instead of
  left-aligning it at octet 0 — the competing reading, and a no-op for the 14-octet counter-mode
  salt — leaves RFC 3711 §B.3's KDF vector passing, every RFC 7714 transform vector passing, and
  **all 448 tests of `sipx-rtp` and `sipx-media` passing**. The same tree fails the browser run in
  the `AEAD_AES_256_GCM` role while the counter-mode role stays green in that same run.

  Two of four combinations remain, and `tests/interop/` cannot close them today: the pinned peer is
  **built without AEAD-GCM at all**. Measured twice — its SRTP module references no
  `srtp_crypto_policy_set_aes_gcm_*`, and offered one `a=crypto` line at a time it answers `200 OK`
  to `AES_CM_128_HMAC_SHA1_80` and `488 Not Acceptable Here` to both GCM suites. So
  `a_real_peer_accepts_media_sipx_encrypted_with_sdes` has always been a counter-mode fact.
  `AEAD_AES_128_GCM`, and both suites over SDES, need a SIP peer that is built with GCM and can be
  made to *require* it; recorded in `tests/interop/README.md` and `docs/specs/srtp.md` §12.10.

  A candidate for that peer was found and verified on the wire — `holius/baresip:v2` answers both
  GCM suites over SDES and completes a GCM DTLS-SRTP handshake in both roles — with two caveats
  recorded in `tests/interop/README.md`: it is a personal-namespace 833 MB image, and it never
  *originates* a GCM offer. **The 128-bit suite is blocked on us, not on it:** sipx offers
  strongest-first and no public API narrows a call's offer to one suite, so `DialOptions` would
  need what `Capabilities::with_srtp_suites` already does a layer down. That is an API decision
  this story should not take on its own.

## Notes

- `tests/interop/run.sh` already supplies peer discovery by `*/profile.sh`, pinned public images,
  per-run certificates from `sipx-testkit --example issue-certs`, and a dynamic CI matrix. The
  harness is not the gap; a peer that speaks AEAD-GCM is.
- This is the same class of gap as `T-13`: an interop claim that cannot be self-proved. Unlike
  `T-13`, a third-party implementation certainly exists here — AEAD-GCM SRTP is widely deployed —
  so this one is achievable without inventing a peer.
- Until this lands, the release notes must not claim AEAD interoperation, only AEAD negotiation and
  transform correctness against the RFC's vectors.
