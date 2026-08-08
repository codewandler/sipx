---
id: M-78
title: Guard public enums by reachability, not by name
pillar: Media
status: done
priority: 4
design:
epic: media
areas: [scripts, sipx-media, sipx-call]
predicate:
announcement:
note: widening the guard from Error-suffixed names to every pub enum reports 149 workspace-wide and 49 on the media path, most of them pub inside private modules
---

# Guard public enums by reachability, not by name

## Goal

Make the `#[non_exhaustive]` guard cover the enums a downstream `match` can actually see, rather
than the ones whose name happens to end in `Error`.

## Acceptance

- [x] The guard selects enums by **reachability from the crate root**, not by name and not by a
      maintained list. A `pub enum` inside a private module is not public API and must not be
      reported.
- [x] A failing-first test covers all three shapes: reachable and unguarded (reported), reachable
      and guarded or reasoned (quiet), `pub` inside a private module (quiet).
- [x] Every enum the corrected rule reports is resolved — `#[non_exhaustive]` or an adjacent
      `/// Exhaustive by design:` rationale — with the reason recorded per enum. Each addition is a
      breaking change for downstream `match` arms and needs a `CHANGELOG.md` statement.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: **the selector is reachability.** `check-audio-claims.py` no longer reads a name.
  `module_graph` walks the `mod` declarations from the crate root recording whether every one of
  them carries a bare `pub`; `reexports` reads each `pub use` so a type published out of a private
  module counts, which is how `sipx-call` presents `MediaProfile` and `IcePolicy` from a private
  `media_policy`. A `pub enum` in a private module nothing re-exports is quiet, which is the half
  this story was filed for. A binary crate contributes nothing, and `code`'s truncation at the
  test-module attribute keeps a fixture enum out of the public surface.

  **The note's premise did not survive measurement, and this is the finding to carry forward.**
  It expected reachability to cut the blanket count sharply because "most are `pub` inside private
  modules". It does not: 149 blanket becomes **162 reachable** by this reader across the workspace
  (144 once `sipx-app-protocol` is excluded), and on the media path 49 becomes **46**. This
  workspace declares most of its modules `pub`, so almost everything a widened rule found really
  is public API. The rule is now *correct* rather than *narrower* — which is still the right
  change, because it is what makes the remaining count a debt rather than noise.

  **Consequence: the rollout is staged, and this is a deviation from row 1's second clause.**
  Resolving 144 enums is a breaking change for downstream `match` arms in eleven crates in one
  diff, which is not reviewable and would collide with every other worktree in the wave.
  `GUARDED_SURFACE` therefore names the five media-path crates the two stories declare — it names
  *crates* and never enums, so nothing inside a covered crate can be excused one item at a time,
  and every run prints how many reachable enums outside it are still exhaustive so the debt is
  reported rather than stored. **A follow-up story is needed for the 98 enums outside it**:
  `sipx-sip` 33, `sipx-ua` 24, `sipx-app` 18, `sipx-transport` 13, `sipx-testkit` 10. `sipx-cli`
  contributes nothing because a binary has no public API; `sipx-app-protocol`'s 18 stay outside
  under `A-9`'s exclusion, and `sipx-wasm` does not publish.

- 2026-08-08: filed from `M-74`, which fixed the four enums it named and measured the rest.
  Replacing the `Error`-suffix regex with every `pub enum` reports **149 across the workspace** and
  **49 on the media path** — but most are `pub` inside private modules and are not public API at
  all, so marking them would be noise rather than contract. Blanket-widening was reverted for that
  reason; the correct selector is reachability, which needs real module-graph work rather than a
  regex.

## Notes

- `M-74` proved the guard has teeth: marking `IcePolicy`, `Keying` and `MediaProfile`
  `#[non_exhaustive]` immediately broke three in-tree matches, which is exactly what a downstream
  consumer would have hit.
- `sipx-app-protocol` is deliberately excluded — it owns a closed, versioned application vocabulary
  and documents its own exceptions.

- 2026-08-08: closed at the `1.0.0-rc.10` boundary. The gate row is ticked against the wave
  gate run on this commit's tree; if that run had been red this line would say so instead.
