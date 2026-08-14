---
id: M-72
title: Prove the AEAD SRTP key derivation against an independent peer
pillar: Media
status: done
priority: 49
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
- [x] Both `AEAD_AES_128_GCM` and `AEAD_AES_256_GCM` are covered, over SDES and over DTLS-SRTP,
      because the two keying paths reach the derivation differently.
- [x] The peer, its exact revision and the negotiated profile are recorded as run evidence a
      stranger can audit, in the shape `tests/interop/` already uses.
- [x] A failing-first negative proves the harness would actually catch a wrong derivation — for
      example a deliberately perturbed salt offset must fail the run, not merely log.
- [x] `./scripts/gate.py` green, with the interop job registered as a gate step or in
      `NOT_RUN_LOCALLY` with a reason.

## Progress

- 2026-08-08: filed from `M-41`'s handoff, as its first stated risk. RFC 7714 publishes no KDF
  vector, so where the 96-bit master salt sits in the PRF input block rests on a reading of the
  spec rather than on a number — recorded at `docs/specs/srtp.md` §4.3 and §12.10. If that reading
  is wrong, two sipx endpoints interoperate with each other and with nobody else, and **every
  round-trip test in the tree still passes**, because both ends share the same mistake. This is the
  failure shape §12.1 already describes.

- 2026-08-14: **both AEAD derivations are independently proved over DTLS-SRTP.** `MediaPolicy` now
  has an exact-suite requirement which narrows both the SDP offer and the DTLS `use_srtp` list to
  one profile and fails closed with plain or automatic keying. Omitting it preserves the existing
  strongest-first behaviour. The bounded native-browser job requires `AEAD_AES_128_GCM` in the
  `browser-offerer` role and `AEAD_AES_256_GCM` in the `browser-answerer` role. A real run carried
  non-silent Opus in both directions under each exact suite. The evidence validator requires the
  browser negotiation, call policy and installed SRTP context all to name that role's suite, and
  captures the independent peer's revision dynamically in the run artifact.

  The negative is executable and measured. `build-perturbed-kdf.py` copies the source tree into an
  owned disposable directory, verifies and replaces one exact KDF fragment only in that copy,
  builds a separate proof endpoint under a five-minute bound, records the original source,
  perturbed source and binary hashes, then removes the copy after its whole process group exits.
  The published SRTP crate contains no selectable broken-crypto path. With the 12-octet AEAD salt
  deliberately right-aligned in RFC 3711's 14-octet `x` value, ICE nomination and DTLS complete,
  both peers send SRTP, neither accepts a packet, and sipx records SRTP authentication failures.
  The proof refuses the negative unless every one of those facts is present and the executable
  matches its build manifest.

  **Both AEAD suites are now independently proved over SDES as well.** The comparison-owned
  wire-evidence registry pins the subject artifact by immutable digest and retains its exact
  configuration, a standalone sipx adapter, results and peer logs entirely within the comparison
  scope. `AEAD_AES_128_GCM` and `AEAD_AES_256_GCM` each negotiated as the sole RFC 4568 suite and
  carried non-silent, partly bit-exact PCMU audio in both directions. A fresh SDES negative used a
  separately built wrong-salt-alignment endpoint: the exact suite still negotiated and 50 packets
  were sent, but sipx accepted none, both implementations recorded authentication failures and the
  independent endpoint rejected all 50. The offline comparison checker validates the closed schema,
  exact-suite facts, source/artifact hashes and negative count without Docker or network access;
  the explicit refresh command is bounded and cancellation-safe. Only the full gate row remains.

- 2026-08-14: the resealed evidence and integrated 55-step repository gate passed.

## Notes

- `tests/interop/run.sh` already supplies peer discovery by `*/profile.sh`, pinned public images,
  per-run certificates from `sipx-testkit --example issue-certs`, and a dynamic CI matrix. The
  harness is not the gap; a peer that speaks AEAD-GCM is.
- This is the same class of gap as `T-13`: an interop claim that cannot be self-proved. Unlike
  `T-13`, a third-party implementation certainly exists here — AEAD-GCM SRTP is widely deployed —
  so this one is achievable without inventing a peer.
- Release prose must retain the evidence scope: exact pinned calls prove both AEAD profiles over
  DTLS-SRTP and RFC 4568 SDES, not SRTCP, rollover, re-keying or universal compatibility.
