---
id: M-83
title: Extend the extensibility guard past the media path
pillar: Media
status: ready
priority: 9
design:
epic: media
areas: [sipx-sip, sipx-ua, sipx-app, sipx-transport, sipx-testkit]
predicate:
announcement:
note: 98 reachable public enums outside the guarded five crates · the boundary names crates, so no enum is individually excused
---

# Extend the extensibility guard past the media path

## Goal

Bring the remaining reachable public enums under the rule `M-78` established, so the guarded set is
the whole published surface rather than the five crates the first pass could review in one diff.

## Why

`M-78` replaced a name-based selector with reachability from the crate root, and `M-74` resolved
what that surfaced on the media path: 46 enums, 25 now `#[non_exhaustive]` and 21 carrying a written
argument for being exhaustive. The measurement also killed the story's premise — this workspace
declares most modules `pub`, so reachability barely narrows the set. Workspace-wide it is 162.

The rest were left outside `GUARDED_SURFACE` deliberately, because resolving 144 enums across eleven
crates in one diff is unreviewable and would have collided with every other branch in flight. The
boundary names **crates and never enums**, so nothing is individually excused, and every run prints
the outstanding count. This story is what closes it.

Remaining: `sipx-sip` 33, `sipx-ua` 24, `sipx-app` 18, `sipx-transport` 13, `sipx-testkit` 10.
`sipx-cli` is a binary and has none; `sipx-app-protocol`'s 18 stay excluded under `A-9`; `sipx-wasm`
does not publish. The cross-crate breakage in the first pass was 14 sites in 6 files — far smaller
than it sounds, so this may be cheaper than the count suggests.

## Acceptance

- [ ] Every reachable public enum in the five named crates is `#[non_exhaustive]` or carries an
      adjacent `/// Exhaustive by design:` argument, with the reason recorded **at the type**.
- [ ] `GUARDED_SURFACE` is retired, or reduced to exactly the crates with a stated reason for being
      outside it. The checker's outstanding count reaches zero, or prints what remains and why.
- [ ] Each enum that becomes `#[non_exhaustive]` is a breaking change for exhaustive `match` arms
      and gets one `CHANGELOG.md` statement covering the set, saying what a downstream arm must add
      and what it buys.
- [ ] A failing-first proof: with the boundary widened and the enums untouched, the checker reports
      them; after, it is quiet.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `M-78`'s own measurement, which is the only reason the size of this is
  known rather than guessed.
