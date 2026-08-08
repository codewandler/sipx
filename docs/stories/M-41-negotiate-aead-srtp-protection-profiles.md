---
id: M-41
title: Negotiate AEAD SRTP protection profiles
pillar: Media
status: in-progress
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

- [x] The SRTP protection profile is a negotiated type carried end to end from the keying path into
      the SRTP context, with key and salt lengths derived from it. No implicit per-profile constants
      remain — a failing-first test proves the wrong lengths cannot be paired with a profile.
- [x] `AEAD_AES_128_GCM` and `AEAD_AES_256_GCM` are implemented for RTP and RTCP per RFC 7714,
      verified against the RFC's own test vectors, recovered from the RFC rather than transcribed —
      matching what `import-rfc4475-corpus.sh` does — with a check that proves the vectors were not
      hand-edited. If they are not machine-extractable, the story states how they were obtained and
      what guards them.
- [x] SDES crypto-suite parsing and generation (RFC 4568) and the DTLS-SRTP `use_srtp` profile list
      (RFC 5764 §4.1.2) both carry the new profiles, ordered strongest-first.
- [x] Selection is **by strength, never by peer order**, matching the rule
      `crates/sipx-ua/src/auth.rs` applies to digest algorithms, with a test that a weaker profile
      offered first does not win.
- [x] `AES_CM_128_HMAC_SHA1_80` remains supported and remains the interoperability floor RFC 5764
      requires. SDES is still offered only over secure signalling.
- [x] Replay window, ROC inference, and authenticate-before-decrypt ordering are shown to hold for
      AEAD **by the existing tests extended to the new profiles**, not by parallel new ones.
- [x] Every path that sizes a buffer from a payload length is audited for the changed tag length, and
      the MTU refusal in `crates/sipx-transport/src/endpoint.rs` is re-derived rather than assumed.
- [x] `docs/rfc/registry.toml` gains RFC 7714 and the RFC 3711 and 5764 rows are updated **in the same
      commit**; `rfc-report.py --check` green.
- [ ] `./scripts/gate.py` green. — every step but one. `maturity` is red because
      `docs/maturity.md` is generated from the RFC registry and this story added a row to it
      (82 → 83 tracked, media partial 16 → 17). That file is fenced for this story, and it is
      fenced for a good reason: it is derived from *every* story's frontmatter, so regenerating it
      here would bake concurrent implementors' in-flight statuses into this commit. One command at
      integration closes it: `./scripts/maturity.py`.

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

- 2026-08-08: **implemented on `impl/M-41`.** The audit's four corrections all held, and the shape
  the work took follows from them.

  **The seam.** `srtp::Profile` is now the negotiated type: an argument to `Context::new`, a field
  of `SrtpKeys`, and what `srtp_context` reads. Two functions and only two turn a negotiated name
  into a cipher — `sipx_media::transform_of` for an RFC 4568 crypto-suite, `dtls::Profile::transform`
  for an RFC 5764 profile. `sipx-sdp` gained no dependency on `sipx-rtp`; the two enums stay
  separate and map at the media layer, so the core crate's layering is unchanged.

  **The list change was the largest piece, as predicted.** `Capabilities::crypto` is a
  `Vec<Crypto>`, strongest first, one key per suite; `MediaDescription::crypto()` is a
  max-by-strength fold with `crypto_offers()` beside it for the whole list; the answerer ranks over
  the *intersection* of what was offered and what it keyed, not over the offer alone.
  `Crypto::accepting` now refuses a suite mismatch as well as a length mismatch — necessary because
  `AES_CM_128_HMAC_SHA1_80` (30 octets) and `AEAD_AES_128_GCM` (28) encode to `inline` parameters
  of the same base64 length, so length is not identity.

  **The vectors are machine-extractable**, though not the way RFC 4475 is: RFC 7714 embeds no
  archive, so `scripts/import-rfc7714-corpus.sh` slices §16 and §17 out of the RFC editor's text,
  strips only the running page furniture, and `--check` re-slices and diffs. It is gate step 40, in
  the `corpus` CI job beside the other two, and disclaims via `EX_TEMPFAIL` when the RFC editor is
  unreachable. All ten published vectors reproduce, including §17.3's tagging-only form.

  **The tag-length audit found nothing to change**, and that is written down rather than left
  silent (`docs/specs/srtp.md` §12.11): the MTU refusal is RFC 3261 §18.1.1 SIP signalling sizing
  on a different socket, nothing outside `sipx-rtp` referenced `TAG_LEN`, `protect` returns a grown
  `Vec` so no caller sizes a tag buffer, and the 2048-octet receive buffers hold 188 octets as
  easily as 182. The only two hardcoded `10`s were assertions in `crates/sipx-media/tests/srtp.rs`.

  **Both AEAD profiles ship**, because RFC 7714 §12 requires both of any implementation and the
  marginal cost is one branch on the key length.

  **One parameter is open and stated rather than hidden.** RFC 7714 publishes no KDF vector, so the
  96-bit master salt's alignment in the AES-CM PRF input block rests on a reading (spec §4.3), and
  if that reading is wrong two sipx endpoints interoperate with each other and with nobody else —
  the §12.1 failure shape, invisible to every round-trip test. §12.10 records it with an owner and
  says the evidence has to be an interop run against an independent implementation, not another
  test in this repository.

  **Gate: 39 of 40 green.** `maturity` is red and the cause is mechanical — `docs/maturity.md` is
  generated from the RFC registry, which this story added a row to. The file is fenced for this
  story. `./scripts/maturity.py` at integration closes it.

## Notes
- Whether `AEAD_AES_256_GCM` earns its place or `128` alone closes the practical gap is a call this
  story makes with evidence; the design does not pre-empt it.
- Unblocks reaching browser-adjacent peers, so it should land before or with `M-38`.
- The negotiation-truth rule from `docs/designs/media-runtime-safety.md` is binding here: never install
  a different cipher under a negotiated identifier.
