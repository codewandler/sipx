---
id: M-73
title: Align the DTLS profile names with the IANA registry
pillar: Media
status: ready
priority: 30
design: docs/designs/media-security-profiles.md
epic: media-security-profiles
areas: [sipx-media, docs]
predicate:
announcement:
note: the counter-mode DTLS profile carries OpenSSL's spelling rather than the registry's; M-41 added registry-correct names beside it
---

# Align the DTLS profile names with the IANA registry

## Goal

Make the DTLS-SRTP protection profile names in the tree the ones the registry defines, so the two
spellings now sitting side by side do not become a permanent inconsistency.

## Acceptance

- [ ] The counter-mode profile is named as the IANA registry names it, matching the AEAD profiles
      `M-41` added from RFC 7714 §14.2.
- [ ] `docs/specs/srtp.md` §12.4 no longer records the divergence as open.
- [ ] A failing-first test pins the wire-visible name, and the change is stated as a behaviour
      change for the `dtls` feature in `CHANGELOG.md` with migration guidance.
- [ ] `./scripts/gate.py` green, and the interop suite is run or its absence stated.

## Progress

- 2026-08-08: filed from `M-41`'s adjacent findings. It deliberately did not widen §12.4 — the two
  names it added are RFC 7714's own — and recorded that renaming the counter-mode row is a
  behaviour change belonging to whoever owns that section.
