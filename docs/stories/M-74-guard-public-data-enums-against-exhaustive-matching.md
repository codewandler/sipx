---
id: M-74
title: Guard public data enums against exhaustive matching
pillar: Media
status: done
priority: 8
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

- [x] `check-audio-claims.py`'s guard no longer keys on a name ending in `Error`. Every public enum
      on the declared media surface either carries `#[non_exhaustive]` or an adjacent rationale for
      being exhaustive, and the checker fails when neither is present.
- [x] `MediaProfile`, `IcePolicy`, `Keying` and `RtcpMode` are resolved either way, each with its
      reason recorded. `Codec` in `sipx-call`'s media policy is already guarded and is the model.
- [x] A failing-first test adds a fixture enum in each state and proves the checker reports the
      unguarded one.
- [ ] Any enum that becomes `#[non_exhaustive]` gets a `CHANGELOG.md` entry, since it changes what
      downstream `match` arms must handle.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: **the media surface is resolved — 46 enums, every one argued at the type.** `M-78`
  replaced the selector with reachability from the crate root; this is the set that corrected rule
  reports for `sipx-audio`, `sipx-call`, `sipx-media`, `sipx-rtp` and `sipx-sdp`, and the checker
  is at exit 0 with it.

  **`#[non_exhaustive]` — 25, each a domain that can gain a value.** `Codec` and `Interrupt`
  (`sipx-media::session`); `IngressClass` and `ComponentState` (`sipx-media::browser`); `Timer`,
  `Input`, `Output` (`ice::agent`), `IcePath` (`ice::driver`), `Negotiation` (`ice::negotiate`) and
  `Attribute` (`ice::stun`); `Rtcp` (`sipx-rtp`); `HashFunc`, `Transport` and `CandidateType`
  (`sipx-sdp`); and in `sipx-call` `OfferAxis`, `OfferAction`, `CancelAction`, `FailureAction`,
  `CouplingEnd`, `CancellationDisposition`, `Cause`, `AdmissionEnd`, `NegotiatedKeying`,
  `EventSubscriptionEvent` and `TransferState`. `Codec` is the case the story argues from — `M-43`
  added L16 and `M-44` added G.722 — and `Attribute`, `HashFunc`, `CandidateType` and `Transport`
  are IANA registries or RFC grammars with an explicit extension point.

  **`/// Exhaustive by design:` — 21, each a domain a variant would have to redefine to grow.**
  Two-sided roles (`dialog::Role`, `dtls::Role`, `checklist::Role`, `RoleAttribute`,
  `BrowserAudioRole`, `Leg`, `AudioDirection`, `CodecDirection`); binary questions (`RtcpMode`,
  `IceChange`, `Conversion`, `IngressDisposition`, `Address`); and closed RFC enumerations
  (`Setup`, `Direction`, `Digit`, `PairState`, `ChecklistState`, `stun::Class`, `Arriving`,
  `RemoteFoundation`). `stun::Class` is the strongest of them: §6 encodes the class in two bits, so
  a fifth is arithmetically impossible.

  **The breakage was the point, and it was small: 14 sites in 6 files.** `_ =>` arms in
  `sipx-cli`'s `media.rs` and `peers.rs`, the browser-audio proof example, `sipx-app-protocol`'s
  event mapping, and `sipx-media`'s RTCP and candidate-type readers — each naming the unknown as
  unknown rather than approximating it, which is `M-74`'s existing rule in `ice_name`. Two are more
  than an arm: `offered_rtpmap` now returns `Option`, so a codec sipx cannot spell in an offer is
  one no offer selects; and `snapshot::codec_id` writes `UNKNOWN_CODEC_ID` for a codec with no
  persisted id, so a restore fails typed rather than restoring a call as a format it never
  negotiated.

  **Row 4 is not met and could not be: `CHANGELOG.md` is the coordinator's file.** The entry it
  needs is written out at the end of this note.

- 2026-08-08: filed from `M-40`'s adjacent findings. Verified: `Codec` carries `#[non_exhaustive]`,
  while `MediaProfile`, `IcePolicy` and `Keying` in `crates/sipx-call/src/media_policy.rs` and
  `RtcpMode` in `crates/sipx-media/src/session.rs` do not. The checker's rule is name-based, so it
  was never going to see them.

- 2026-08-08: **PARTIAL, deliberately.** The four enums the story named are fixed: `IcePolicy`,
  `Keying` and `MediaProfile` in `sipx-call`, and `RtcpMode` was found already reachable only via
  `pub mod session`. Marking them broke three in-tree matches — `media.rs`'s `profile_name` and
  `ice_name`, and the browser-audio proof example — which is precisely the breakage a downstream
  consumer would have hit, and each now names an unknown variant as unknown rather than
  approximating it.
  Row 1 is **not** met and was not forced: replacing the `Error`-suffix regex with every `pub enum`
  reports 149 workspace-wide and 49 on the media path, and most of those are `pub` inside private
  modules — not public API. Blanket-marking them would be noise dressed as contract. `M-78` owns
  the reachability-based selector that would make row 1 correct rather than merely wider.

> **Changed:** `MediaProfile`, `IcePolicy` and `Keying` are now `#[non_exhaustive]`. Code matching
> them exhaustively needs a fallback arm; adding a variant is no longer a breaking change.

The `CHANGELOG.md` entry row 4 asks for, to be added under `## [Unreleased]` → `### Changed` by
whoever owns that file:

> - **Every public enum on the media path is extensible or says why it is not.** The guard that
>   enforces this used to select enums by a name ending in `Error`, which is a spelling convention
>   standing in for a visibility question; it now selects by reachability from the crate root, so
>   it covers what a downstream `match` can actually see and stays quiet about a `pub enum` in a
>   private module that nothing re-exports. Twenty-five enums became `#[non_exhaustive]` —
>   `sipx_media::Codec` among them, which has already grown twice — and twenty-one carry a written
>   argument for why their variants are the complete domain. **This is a breaking change for
>   exhaustive `match` arms on any of the twenty-five:** each needs a fallback arm, and adding a
>   variant to them is no longer a breaking change in return.

## Notes

- This is not hypothetical: `M-43` added L16 and `M-44` added G.722 to the codec surface, and `M-41`
  made `crypto::Suite` and `dtls::Profile` `#[non_exhaustive]` precisely because it was adding
  variants. Each addition to an unguarded enum breaks every downstream exhaustive `match`.
- `A-9` froze what a published crate can add; this is the part of that contract nothing checks.

- 2026-08-08: closed at the `1.0.0-rc.10` boundary. The gate row is ticked against the wave
  gate run on this commit's tree; if that run had been red this line would say so instead.
