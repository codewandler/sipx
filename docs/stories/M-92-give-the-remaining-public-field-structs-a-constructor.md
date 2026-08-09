---
id: M-92
title: Give the remaining public-field structs a constructor, then mark them
pillar: Media
status: in-progress
priority: 8
design:
epic: media
areas: [scripts, sipx-media, sipx-rtp]
predicate:
announcement:
note: M-80's struct guard holds 6 types; 155 reachable public-field structs workspace-wide and 29 on the media surface are still literal-constructible
---

# Give the remaining public-field structs a constructor, then mark them

## Goal

Close the gap `M-80` measured: every reachable public struct on the media surface whose fields are
public should be `#[non_exhaustive]`, which for most of them means designing a constructor first.

## Why

`M-80` settled `#[non_exhaustive]` for `sipx_media::Encoded` and `sipx_rtp::Packet`, and made the
rule enforceable in `scripts/check-audio-claims.py`. That rule runs behind two boundaries, both
stated at their definitions:

- `MEDIA_SURFACE` names `sipx-media` and `sipx-rtp`, the two crates the relay path runs through.
- `breakable_structs` holds only a struct the crate *already publishes a `new` for*, because
  `#[non_exhaustive]` on a struct with no constructor leaves a downstream caller no way to build
  one at all.

The second boundary is what this story is against. Six types satisfied it. The run prints the
remainder on every gate — 155 reachable public-field structs workspace-wide, 29 of them in the two
guarded crates — and every one of those is a type where a field addition is still a silent breaking
change for a downstream struct literal, which is exactly what `M-75` and `M-79` did twice.

The deadline is the same one `M-80` was filed against and it has not moved: `#[non_exhaustive]` can
be removed in a minor release and only added in a major one, so every type still unmarked when
`1.0.0` is cut is decided for the life of the major version. `docs/roadmap.md`'s v1 predicate 4 is
the point after which the contract stops being editable to fit the change.

The largest single group is `sipx-rtp`'s RTCP types — `ReportBlock`, `SenderReport`,
`ReceiverReport`, `SdesItem`, `SdesChunk`, `Sdes` — which are wire shapes with no constructors at
all, and `sipx-media`'s ICE candidate and checklist types.

## Acceptance

- [x] Every reachable public-field struct in `sipx-media` and `sipx-rtp` either publishes a
      constructor and carries `#[non_exhaustive]`, or carries an adjacent `/// Complete by design:`
      rationale saying why its field set is the whole record. All 29; a third form was needed for
      two of them and is described in `## Progress`.
- [x] Each new constructor is argued rather than mechanical: a wire type whose fields are all
      required takes them all, and one with a defaultable tail does not.
- [x] The `breakable_structs` type boundary in `scripts/check-audio-claims.py` is **removed**, so
      the rule reads "reachable, public fields" on the media surface with no further narrowing —
      or, if it is kept, the checker states what it is still for.
- [x] The outstanding count the run prints falls to zero for `MEDIA_SURFACE`, and the remaining
      workspace figure is unchanged in meaning.
- [ ] `CHANGELOG.md` says what a downstream literal should become, per type. — the sentence is
      written out in `## Progress`; `CHANGELOG.md` is the coordinator's to write.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed by `M-80`, which measured the remainder rather than estimating it. The number
  is printed by `./scripts/check-audio-claims.py --check` and is a gate output, so it does not need
  restating here to stay current.

- 2026-08-09: delivered on `impl/M-92`. The rule was changed first and run against the untouched
  tree, which reported all 29 and named them:

  ```
  $ ./scripts/check-audio-claims.py --check   # EXIT=1
  the crate front doors advertise what the crates do not implement, or do not say what they guarantee:
    crates/sipx-media/src/browser.rs:210 `IngressCounts` is reachable from the crate root and has public fields; add `#[non_exhaustive]` or an adjacent `/// Complete by design:` rationale
    … 27 more …
    crates/sipx-rtp/src/rtcp.rs:116 `Sdes` is reachable from the crate root and has public fields; add `#[non_exhaustive]` or an adjacent `/// Complete by design:` rationale
  ```

  **The type boundary became an obligation rather than being deleted.** `M-80` narrowed the rule to
  structs that already published a `new`, because `#[non_exhaustive]` on one that publishes nothing
  leaves a caller no way to build it. That hazard is real, and using it to drop 29 types out of the
  rule was not. So `breakable_structs` no longer narrows, and `struct_problems` now charges the
  attribute's cost to whoever takes it: a marked struct must publish a constructor, or carry a
  `/// Built by this crate only:` rationale. `publishes_a_constructor` was widened to match — any
  public associated function returning `Self`, plus a derived or hand-written `Default`, because
  `T::default()` and an assignment reach every value a literal could and a rule that only accepted
  `new` would have demanded a worse-named second function beside `Sdes::cname`,
  `SrtpKeys::from_answer` and `RemoteCandidate::signalled`.

  **`MediaDiscardCounts` is `#[non_exhaustive]` with no `new`.** It is the type on this surface a
  field arrives at most often — `docs/specs/media-runtime.md` §4 requires every discard site to
  increment a counter or write down why none can reach it, so the set moved twice in two releases
  (`M-81` added one, `M-90` removed it). `Default` is derived and every field is public, so nothing
  is stranded and a twenty-argument constructor would be worse than the literal it replaced. The
  in-crate `total_sums_every_published_field_exactly_once` literal is untouched: the attribute binds
  other crates, not the defining one, so that discipline is unchanged. §4's `M-90` paragraph said
  restoring a removed counter would cost a major release; that is now a minor one, and §4 has been
  rewritten to say so and to say why `M-90`'s decision is unaffected — the field went because
  nothing could move it, not because restoring it was expensive.

  **Three answers, by type.** *Complete by design* (10, no attribute, no constructor): `ReportBlock`
  — RFC 3550 §6.4.1's fixed 24 octets, all seven named, with the derived view living in
  `RtcpQualitySample` instead; `SdesItem`, `SdesChunk`, `Sdes` — §6.5 gives SDES no extension area,
  and the line drawn across `rtcp.rs` is §6.4.3's: the two types the RFC gives a profile-specific
  extension get the attribute and the ones it fixes do not; and the six identity/foundation
  newtypes `LocalBase`, `LocalId`, `RemoteId`, `LocalFoundation`, `PairId`, `PairFoundation`, where
  marking would have cost the tuple literal for no hazard.
  *Marked with a constructor* (17): `SenderReport::new`/`ReceiverReport::new` (sender info in, the
  §6.4.2-empty block list out), `Quality::new` (four measured numbers; `mos` is **derived**, so a
  score that disagrees with its own inputs is unrepresentable), `TonePacket::new`, `Completed::new`,
  `Gathered::new`, `LocalCandidate::new` (priority derived from §5.1.2.1's formula — the peer-
  reflexive provenance is priced from the check's `PRIORITY` and stays the agent's),
  `CandidatePair::new` and `ValidPair::new` (state Frozen and nominated false are §6.1.2.6's and
  §7.2.5.3.4's, not arguments), `Local::new`, `SrtpKeys::new`, `RtcpQualitySample::new`, and
  `IngressCounts`/`MediaDiscardCounts`/`ice::Config`/`ice::Timers` through `Default`, plus
  `RemoteCandidate` through the `signalled` it already had.
  *Marked without a constructor* (2): `dtls::Keys`, whose private `material` already made the
  literal unwritable outside the crate, and `BrowserComponentSnapshot`, a reading taken off a
  `ComponentIngress` that only exists in this crate.

  **The debt line reads 147 and did not fall by 8.** `M-80` computed its `held` set for every crate,
  so a struct *outside* `MEDIA_SURFACE` that happened to publish a `new` was subtracted from the
  debt in a crate where no rule holds it — neither guarded nor counted. 155 was 29 inside plus 126
  outside; 147 is those 126 plus the 21 that subtraction was hiding. `outstanding_structs` now skips
  `MEDIA_SURFACE` outright, because a struct there is either held or a failure.

  Verified in this worktree: `python3 -m unittest scripts/test-audio-claims.py` (103 tests),
  `./scripts/check-audio-claims.py --check`, `cargo check --workspace --all-features --all-targets`,
  `cargo test -p sipx-media -p sipx-rtp --all-features`, `cargo clippy --workspace --all-targets
  --all-features --no-deps -- -D warnings`, `cargo fmt --all`, `cargo doc -p sipx-media -p sipx-rtp
  --no-deps --all-features`. `./scripts/gate.py` was not run here — it is run once per wave.

  **CHANGELOG sentence this story owes** (the coordinator writes the file):

  > Every public-field struct on the media surface is now either `#[non_exhaustive]` with a
  > constructor or documented as complete, closing the 29 types `M-80`'s rule left outside it. A
  > downstream literal becomes: `SenderReport::new(ssrc, ntp, rtp, packets, octets)` then
  > `.reports`; `ReceiverReport::new(ssrc)` then `.reports`; `Quality::new(loss, cumulative_lost,
  > jitter, round_trip)`, which now computes `mos` rather than taking it; `TonePacket::new`,
  > `Completed::new`, `Gathered::new`, `LocalCandidate::new(id, gathered, foundation,
  > local_preference)`, `CandidatePair::new`, `ValidPair::new`, `ice::Local::new`,
  > `SrtpKeys::new(profile, local, remote)` and `RtcpQualitySample::new`; and for
  > `MediaDiscardCounts`, `IngressCounts`, `ice::Config` and `ice::Timers`, `T::default()` followed
  > by assigning the fields — `..Default::default()` no longer applies across a crate boundary.
  > `ReportBlock`, `SdesItem`, `SdesChunk`, `Sdes` and the ICE identity newtypes keep their literals
  > and say why.
