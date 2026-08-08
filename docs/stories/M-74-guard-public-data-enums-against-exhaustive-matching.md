---
id: M-74
title: Guard public data enums against exhaustive matching
pillar: Media
status: ready
priority: 31
design:
epic: media
areas: [sipx-media, sipx-call, scripts]
predicate:
announcement:
note: the non_exhaustive check only matches enums whose name ends in Error, so MediaProfile, IcePolicy, Keying and RtcpMode are unguarded
---

# Guard public data enums against exhaustive matching

## Goal

Stop a new variant on a public media enum from being a silent breaking change. `A-9` argued the case
for error enums and the checker enforces it there; the same argument applies to the data enums on
the media path and nothing enforces it.

## Acceptance

- [ ] `check-audio-claims.py`'s guard no longer keys on a name ending in `Error`. Every public enum
      on the declared media surface either carries `#[non_exhaustive]` or an adjacent rationale for
      being exhaustive, and the checker fails when neither is present.
- [ ] `MediaProfile`, `IcePolicy`, `Keying` and `RtcpMode` are resolved either way, each with its
      reason recorded. `Codec` in `sipx-call`'s media policy is already guarded and is the model.
- [ ] A failing-first test adds a fixture enum in each state and proves the checker reports the
      unguarded one.
- [ ] Any enum that becomes `#[non_exhaustive]` gets a `CHANGELOG.md` entry, since it changes what
      downstream `match` arms must handle.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `M-40`'s adjacent findings. Verified: `Codec` carries `#[non_exhaustive]`,
  while `MediaProfile`, `IcePolicy` and `Keying` in `crates/sipx-call/src/media_policy.rs` and
  `RtcpMode` in `crates/sipx-media/src/session.rs` do not. The checker's rule is name-based, so it
  was never going to see them.

## Notes

- This is not hypothetical: `M-43` added L16 and `M-44` added G.722 to the codec surface, and `M-41`
  made `crypto::Suite` and `dtls::Profile` `#[non_exhaustive]` precisely because it was adding
  variants. Each addition to an unguarded enum breaks every downstream exhaustive `match`.
- `A-9` froze what a published crate can add; this is the part of that contract nothing checks.
