---
id: M-25
title: Specify SRTP and its two keyings, after the fact
pillar: Media
status: in-progress
priority: 9
design: docs/designs/media.md
epic: media
areas: [docs, sipx-rtp]
note: found by X-25 — M-14 and M-15 shipped without the spec non-negotiable 4 requires
---

# Specify SRTP and its two keyings, after the fact

## Goal
Give SRTP, SDES and DTLS-SRTP the spec `AGENTS.md` requires of a non-trivial subsystem, so the
media security path is documented the way the transport and signalling paths are.

## Acceptance
- [x] `docs/specs/` gains a spec covering what `M-14` and `M-15` built: the SRTP transform and its
      key derivation, SDES key exchange, DTLS-SRTP, the profiles supported, and the rules for which
      keying wins when both are offered.
- [x] It carries what a spec in this repository carries: normative RFC references, the types, the
      state involved, and **byte-level test vectors** — the published SRTP vectors are the obvious
      source, and the existing tests should be derived from them or reconciled with them.
- [x] The seven rules `M-14`/`M-15` settled between SDES and DTLS-SRTP are stated normatively
      rather than living only in two closed story files.
- [x] Whether this changes any code is the story's finding, not its premise. Writing a spec after
      the implementation usually surfaces at least one place where the code and the intent differ;
      if it surfaces none, say so explicitly.
- [x] The spec is reachable the way the others are: linked from wherever `docs/specs/` is indexed,
      and cited by the RFC registry rows it covers.

## Progress
- **Spec written: [`docs/specs/srtp.md`](../specs/srtp.md)**, in the shape `docs/specs/ice.md` set.
  §1 normative references, §3 the types across three crates, §4 the transform (parameters, layout,
  key derivation, IV, rollover, tag, SRTCP, replay, key lifetime), §5 SDES, §6 DTLS-SRTP, §7 the
  seven rules, §8 state, §9 what must not happen, §10 vectors, §11 where the code goes, §12 what
  writing it found.
- **Which keying wins when both are offered** is §7 rule 2: the `m=` protocol token decides, because
  one `m=` line carries one `proto` token (RFC 8866 §5.14), so a stream cannot be offered under both.
  A `UDP/TLS/RTP/SAVP` line carrying `a=crypto` is keyed by DTLS-SRTP and the `a=crypto` is not read.
  Offering the same stream twice under two tokens is RFC 5939 capability negotiation, which sipx
  neither offers nor answers.
- **The finding: writing the spec changed code.** Two conformance defects, both fixed in `928a340`,
  both wire-visible:
  - `SESSION_AUTH_LEN` was 94 octets. RFC 3711 §5.2 and §8.2 fix `n_a` at 160 bits; the 94 is §B.3's
    worked example positing an authentication function that needs that much, to walk the PRF through
    six AES blocks. HMAC takes a key of any length, so nothing errored — both ends of a sipx call
    derived the same wrong key and every round-trip test passed, while no conformant peer would have
    authenticated a single packet in either direction. Now 20, asserted against the first 160 bits of
    §B.3's own published block.
  - The SRTCP index was incremented *before* the packet, so the first carried 1 and index 0 was never
    emitted. §3.4 states read-then-advance as a MUST. Not an interoperability failure — the index is
    explicit in the trailer — but it selects the SRTCP keystream's counter block.
- **Three new tests**, two of them the vectors the defects were found by:
  `the_session_authentication_key_is_the_160_bits_the_rfc_fixes`,
  `the_authentication_tag_is_hmac_sha1_over_the_packet_and_the_roc` (HMAC-SHA1 computed off-stack,
  over §B.3's key truncated to `n_a` and §B.1's published header and ROC, for both forms of `M`),
  and `the_first_srtcp_packet_carries_index_zero`. `authenticate` now takes a key slice so a test can
  hand it the RFC's key rather than this module's.
- **`key_derivation_matches_the_rfc` was reconciled, not corrected.** Its 94-octet assertion is right
  — it tests the PRF at §B.3's length, which is where six AES blocks catch a counter that does not
  advance. It now says that is what it tests, and points at the test that covers `n_a`.
- **Three more disagreements found and left open**, each with an owner in §12: SRTCP has no replay
  list (§3.4), the SDES tag is neither echoed (RFC 4568 §5.1.2) nor verified (§5.1.3) — both MUSTs,
  and the protection profile is named in OpenSSL's spelling rather than the RFC's. Two published SDP
  vectors (§10.4's `inline` lines, §10.6's fingerprint lines) are stated and not yet asserted. §12.7
  lists what was checked and agreed, so the list reads as a finding rather than a sample.
- **Reachability:** `docs/designs/media.md` is where the media area indexes its documents — its
  "what lives where" table gains a `specs/srtp.md` row, the "there is no spec" paragraph is replaced,
  and the gap entry in the gaps list is closed with what the omission cost rather than why it
  happened (`X-25` had already looked for the why and found nothing). The five SRTP-family registry
  rows (3711, 4568, 5764, 5763, 8122) cite it through a new optional `spec` key, existence-checked
  like `evidence` and rendered as a Spec column; the two `partial` notes gain the gaps `M-25` found.
- The board, `CHANGELOG.md` and `docs/roadmap.md` are the coordinator's to write; untouched here.

## Notes
- Found by `X-25`: SRTP, SDES and DTLS-SRTP have no spec in `docs/specs/`, which is a standing
  breach of `AGENTS.md` non-negotiable 4 ("Spec before code. Non-trivial subsystems get a spec in
  `docs/specs/` first"). `X-25` also recorded *why* they shipped without one as unrecorded — it
  looked and found nothing.
- The order is inverted and that is worth naming: this is spec-after-code, which the rule exists to
  prevent. It is still worth doing — `M-16` showed what writing ICE's spec first bought (two errors
  caught by the first two implementors rather than by a peer on the wire), and the media security
  path currently has none of that.
- Sibling in kind to `X-25`, which recorded the media *design*. This is the normative half.
