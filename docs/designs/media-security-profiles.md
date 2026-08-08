# Design: media security profiles

**Status:** implemented by `M-41` · **Pillar:** Media · **Epic:** `media-security-profiles` ·
**Stories:** M-41 · **Spec:** [srtp.md](../specs/srtp.md)

## Why

sipx implements exactly one SRTP protection profile:
`AES_CM_128_HMAC_SHA1_80` (`crates/sipx-rtp/src/srtp/mod.rs`). It is correct, checked against the
RFC 3711 Appendix B vectors, authenticates before it decrypts, and is the profile every SIP peer
can be assumed to support. As a floor it is the right choice.

As a ceiling it is a real capability gap. RFC 7714 defines the AEAD profiles
`AEAD_AES_128_GCM` and `AEAD_AES_256_GCM`, and RFC 5764 §4.1.2 registers their DTLS-SRTP
counterparts. Peers that offer only AEAD — increasingly the WebRTC-adjacent ones sipx wants to
reach through `M-38` — cannot negotiate media with sipx at all, and a stack that ships a single
1980s-lineage counter-mode-plus-HMAC profile reads as dated regardless of how well it is
implemented. AEAD also removes the encrypt-then-MAC ordering question entirely, since the tag and
the ciphertext are produced by one construction.

This is a negotiation gap as much as a cipher gap. Both keying paths must learn the new profiles:
SDES (RFC 4568) carries a crypto-suite token per offer line, and DTLS-SRTP (RFC 5764) carries a
profile list in the `use_srtp` extension. Offering a profile sipx cannot honour, or accepting one
it cannot key, is worse than not offering it.

## Approach

- Make the protection profile an explicit negotiated value end to end rather than an assumption:
  one type that names the profile, carried from the keying path into the SRTP context, with the
  key and salt lengths derived from it rather than from constants.
- Implement `AEAD_AES_128_GCM` and `AEAD_AES_256_GCM` per RFC 7714 for RTP and RTCP, checked
  against the RFC's own test vectors the way RFC 3711 Appendix B already is — vectors first, then
  the implementation.
- Extend SDES crypto-suite parsing and generation (RFC 4568) and the DTLS-SRTP `use_srtp` profile
  list (RFC 5764 §4.1.2) to carry the new profiles, ordered strongest-first, with the existing
  rule intact: SDES is offered only over secure signalling.
- Selection is by strength, never by peer order — the same rule
  `crates/sipx-ua/src/auth.rs` already applies to digest algorithms, and for the same reason.
- The replay window, ROC inference and authenticate-before-decrypt ordering are profile-independent
  and must be shown to hold for AEAD by the same tests, not by new ones.

## Alternatives considered

- **AEAD for DTLS-SRTP only, not SDES.** Rejected: it splits the profile set by keying path, so a
  peer's reachable cipher depends on how it happened to key. One profile set, both paths.
- **Add the profiles without making the profile a negotiated type.** Rejected — the constants for
  key and salt length are currently implicit in the single supported profile, and adding a second
  set of implicit constants is how the wrong one gets used under the right label. The negotiation
  truth rule from `docs/designs/media-runtime-safety.md` applies: never install a different cipher
  under a negotiated identifier.
- **Drop `AES_CM_128_HMAC_SHA1_80` once AEAD lands.** Rejected: it is the interoperability floor,
  and RFC 5764 requires it as the mandatory-to-implement profile.

## Risks and open questions

- AEAD changes the packet's size relationship between plaintext and ciphertext (tag length differs
  from the HMAC-80 authentication tag). Every path that sizes a buffer from a payload length needs
  auditing, and the MTU refusal in `crates/sipx-transport/src/endpoint.rs` must be re-derived
  rather than assumed to still hold.
- RFC 7714 vectors must be recovered from the RFC rather than transcribed, matching what
  `import-rfc4475-corpus.sh` does for the torture corpus. Whether the vectors are machine-
  extractable from the RFC text is the open question; if not, the story states how they were
  obtained and the check that proves they were not hand-edited.
- Whether `AEAD_AES_256_GCM` earns its place, or `128` alone closes the practical gap, is a call
  the story makes with evidence rather than a decision this design pre-empts.

## What `M-41` settled

- **The tag length audit found nothing to change.** The MTU refusal is RFC 3261 §18.1.1 signalling
  sizing on a different socket, no buffer anywhere was derived from an SRTP overhead figure, and
  nothing outside `sipx-rtp` referenced `TAG_LEN`. Recorded in `docs/specs/srtp.md` §12.11 so the
  absence is a finding rather than a silence.
- **Both AEAD profiles ship.** RFC 7714 §12: "Any implementation of AES-GCM SRTP MUST support both
  `AEAD_AES_128_GCM` and `AEAD_AES_256_GCM`." Shipping one would have been a conformance claim the
  spec could not make, for a saving of one branch on the key length.
- **The vectors are machine-extractable**, though not as an archive: RFC 7714 embeds none, so
  `scripts/import-rfc7714-corpus.sh` slices §16 and §17 out of the RFC editor's text and `--check`
  re-slices and diffs. `docs/specs/srtp.md` §10.7.
- **One parameter is still unpinned**: RFC 7714 publishes no KDF vector, so the master-salt
  alignment in the AEAD key derivation rests on a reading rather than on a number. Stated in the
  spec (§4.3, §12.10) with the interoperability consequence spelled out, and left to a story whose
  evidence has to come from an independent implementation.
