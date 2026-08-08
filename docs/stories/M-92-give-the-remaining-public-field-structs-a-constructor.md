---
id: M-92
title: Give the remaining public-field structs a constructor, then mark them
pillar: Media
status: backlog
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

- [ ] Every reachable public-field struct in `sipx-media` and `sipx-rtp` either publishes a
      constructor and carries `#[non_exhaustive]`, or carries an adjacent `/// Complete by design:`
      rationale saying why its field set is the whole record.
- [ ] Each new constructor is argued rather than mechanical: a wire type whose fields are all
      required takes them all, and one with a defaultable tail does not.
- [ ] The `breakable_structs` type boundary in `scripts/check-audio-claims.py` is **removed**, so
      the rule reads "reachable, public fields" on the media surface with no further narrowing —
      or, if it is kept, the checker states what it is still for.
- [ ] The outstanding count the run prints falls to zero for `MEDIA_SURFACE`, and the remaining
      workspace figure is unchanged in meaning.
- [ ] `CHANGELOG.md` says what a downstream literal should become, per type.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed by `M-80`, which measured the remainder rather than estimating it. The number
  is printed by `./scripts/check-audio-claims.py --check` and is a gate output, so it does not need
  restating here to stay current.
