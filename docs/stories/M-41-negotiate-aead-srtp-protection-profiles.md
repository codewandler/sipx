---
id: M-41
title: Negotiate AEAD SRTP protection profiles
pillar: Media
status: ready
priority: 3
design: docs/designs/media-security-profiles.md
epic: media-security-profiles
areas: [sipx-rtp, sipx-sdp, sipx-media]
predicate:
announcement:
note: RFC 7714 · one profile shipped today · AEAD-only peers cannot negotiate media with sipx at all · follow-up
---

# Negotiate AEAD SRTP protection profiles

## Goal

Add the RFC 7714 AEAD-GCM protection profiles alongside the shipped counter-mode profile, and make
the profile an explicitly negotiated value on both keying paths, so peers that offer only AEAD can
establish media with sipx.

## Acceptance

- [ ] The SRTP protection profile is a negotiated type carried end to end from the keying path into
      the SRTP context, with key and salt lengths derived from it. No implicit per-profile constants
      remain — a failing-first test proves the wrong lengths cannot be paired with a profile.
- [ ] `AEAD_AES_128_GCM` and `AEAD_AES_256_GCM` are implemented for RTP and RTCP per RFC 7714,
      verified against the RFC's own test vectors, recovered from the RFC rather than transcribed —
      matching what `import-rfc4475-corpus.sh` does — with a check that proves the vectors were not
      hand-edited. If they are not machine-extractable, the story states how they were obtained and
      what guards them.
- [ ] SDES crypto-suite parsing and generation (RFC 4568) and the DTLS-SRTP `use_srtp` profile list
      (RFC 5764 §4.1.2) both carry the new profiles, ordered strongest-first.
- [ ] Selection is **by strength, never by peer order**, matching the rule
      `crates/sipx-ua/src/auth.rs` applies to digest algorithms, with a test that a weaker profile
      offered first does not win.
- [ ] `AES_CM_128_HMAC_SHA1_80` remains supported and remains the interoperability floor RFC 5764
      requires. SDES is still offered only over secure signalling.
- [ ] Replay window, ROC inference, and authenticate-before-decrypt ordering are shown to hold for
      AEAD **by the existing tests extended to the new profiles**, not by parallel new ones.
- [ ] Every path that sizes a buffer from a payload length is audited for the changed tag length, and
      the MTU refusal in `crates/sipx-transport/src/endpoint.rs` is re-derived rather than assumed.
- [ ] `docs/rfc/registry.toml` gains RFC 7714 and the RFC 3711 and 5764 rows are updated **in the same
      commit**; `rfc-report.py --check` green.
- [ ] `./scripts/gate.py` green.

## Progress
- (not started)

- 2026-08-08: **readiness audit — ready, with four corrections for the implementor.** There is half a
  seam: `sipx-sdp::crypto::Suite` and `sipx-media::dtls::Profile` are both single-variant enums that
  already carry `key_and_salt_len()`, but the profile is **discarded at `SrtpKeys`** (only byte pairs
  survive) and `sipx-rtp::srtp` has no profile concept — its cipher is a type alias, its lengths are
  `pub const`, and `derive()` takes fixed-size arrays. The largest single piece is not the ciphers:
  it is turning `Capabilities::crypto: Option<Crypto>` into a strongest-first list and replacing the
  `find_map` with a max-by-strength fold. `aes-gcm` must be added as a RustCrypto dependency, not
  OpenSSL. This story's two file pointers are stale; the spec rewrite is in scope, and so is the
  SDES list change this story never names.

## Notes
- Whether `AEAD_AES_256_GCM` earns its place or `128` alone closes the practical gap is a call this
  story makes with evidence; the design does not pre-empt it.
- Unblocks reaching browser-adjacent peers, so it should land before or with `M-38`.
- The negotiation-truth rule from `docs/designs/media-runtime-safety.md` is binding here: never install
  a different cipher under a negotiated identifier.
