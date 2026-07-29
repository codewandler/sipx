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
- [ ] `docs/specs/` gains a spec covering what `M-14` and `M-15` built: the SRTP transform and its
      key derivation, SDES key exchange, DTLS-SRTP, the profiles supported, and the rules for which
      keying wins when both are offered.
- [ ] It carries what a spec in this repository carries: normative RFC references, the types, the
      state involved, and **byte-level test vectors** — the published SRTP vectors are the obvious
      source, and the existing tests should be derived from them or reconciled with them.
- [ ] The seven rules `M-14`/`M-15` settled between SDES and DTLS-SRTP are stated normatively
      rather than living only in two closed story files.
- [ ] Whether this changes any code is the story's finding, not its premise. Writing a spec after
      the implementation usually surfaces at least one place where the code and the intent differ;
      if it surfaces none, say so explicitly.
- [ ] The spec is reachable the way the others are: linked from wherever `docs/specs/` is indexed,
      and cited by the RFC registry rows it covers.

## Progress
- Not started.

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
