---
id: M-78
title: Guard public enums by reachability, not by name
pillar: Media
status: ready
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

- [ ] The guard selects enums by **reachability from the crate root**, not by name and not by a
      maintained list. A `pub enum` inside a private module is not public API and must not be
      reported.
- [ ] A failing-first test covers all three shapes: reachable and unguarded (reported), reachable
      and guarded or reasoned (quiet), `pub` inside a private module (quiet).
- [ ] Every enum the corrected rule reports is resolved — `#[non_exhaustive]` or an adjacent
      `/// Exhaustive by design:` rationale — with the reason recorded per enum. Each addition is a
      breaking change for downstream `match` arms and needs a `CHANGELOG.md` statement.
- [ ] `./scripts/gate.py` green.

## Progress

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
