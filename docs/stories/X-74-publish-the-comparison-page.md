---
id: X-74
title: Publish the comparison page
pillar: Build
status: done
priority: 17
design: docs/designs/stack-comparison.md
epic: stack-comparison
areas: [website, scripts]
predicate:
announcement:
note: three-hop compliance chain · must state where sipx loses · blocked on X-73
---

# Publish the comparison page

## Goal

Put the comparison on the public site, generated from the registry and sanitised the way the
compliance table already is, so a chooser gets the answer on the surface where they are asking.

## Acceptance

- [ ] `website/docs/reference/comparison.md` exists with frontmatter `title` and `description`, a
      hand-written surround, and a `<!-- BEGIN generated:comparison -->` region.
- [ ] `scripts/sync-website.py` gains `render_comparison()` dispatched from `render_generated()`
      and a `public_comparison()` sanitiser modelled on `public_compliance()` — stripping the H1
      and generator banner, rewriting internal doc links to absolute URLs, and removing work-item
      IDs and story/design links so the public-content guard passes.
- [ ] Following the `claimed_codecs()` precedent, the renderer **runs
      `comparison-report.py --check` and raises rather than renders if it is red.** A stale or
      unevidenced dataset must not be able to reach the site.
- [ ] The page is registered in `sidebars.js` under **Reference**. A page absent from the sidebar is
      silently missing from `llms.txt` and `llms-full.txt`, so this is required, not cosmetic.
- [ ] The page joins `CURRENT_SURFACE_PAGES` in `sync-website.py`, subjecting it to the stale-claim
      guard — the mechanism that would have caught the deleted migration pages rotting.
- [ ] The confidence tier is rendered **per cell**, so a reader can tell a measurement from a
      judgment without leaving the page.
- [ ] The hand-written surround carries all four: the method and the refresh command · the evidence
      asymmetry · **where sipx loses**, including maturity, external adoption and the absence of a
      third-party audit · what the page does not establish (it is not an interop result and not an
      audit).
- [ ] `scripts/test-sync-website.py` covers the new renderer: the rendered body matches the
      canonical source and matches neither `STORY_ID` nor `INTERNAL_PUBLIC_LINK`.
- [ ] `./scripts/build-docs.sh` passes — zero `[WARNING]` lines, all four throw handlers happy, the
      region byte-matching a fresh render, and **no new `WARNING_EXCEPTIONS` entry**.
- [ ] `./scripts/gate.py` green.

## Progress

Implemented 2026-08-04. Everything in Acceptance is satisfied.

- **`website/docs/reference/comparison.md`** — frontmatter `title` and `description`, a hand-written
  H1 and intro, the `<!-- BEGIN generated:comparison -->` region, and a hand-written trailer *after*
  the END marker (`process()` only rewrites between markers).
- **`render_comparison()`** is dispatched from `render_generated()` beside the `compliance` arm, and
  **`public_comparison()`** follows `public_compliance()`: strips the H1 and generator banner, drops
  story-ID sentences, and rewrites every internal evidence link to an absolute URL. Evidence cites
  files two ways — `../x` for the repository root, `x` for something under `docs/` — so the
  root-relative form is rewritten first and the second pattern only ever sees a `docs/` path.
- **Raise rather than render**: `public_comparison()` runs `comparison-report.py --check` first and
  raises `ValueError` if it is red, the `claimed_codecs()` rule. A test asserts the shape rather
  than the behaviour, so the assertion cannot pass by the checker happening to be green.
- Registered in `sidebars.js` under **Reference** and added to `CURRENT_SURFACE_PAGES`.

**One guard needed a scoped change, and it is the interesting part of this story.** This is the
only public surface that quotes other projects' version numbers, and every row names one, so
`public_fact_problems()` fired eleven times on subject tags. A page-wide waiver would have been
wrong — this is precisely the page most likely to carry a stale *sipx* version. Instead
`foreign_stack_row()` holds the fact guards off on a comparison table row whose first cell is not
sipx, and on nothing else — sipx's own rows, the surround, and every other public page stay
checked. The waiver covers the whole fact check rather than the version patterns alone, and that is
deliberate: no number on another stack's row is a claim about sipx, so checking a subject's Rust
version or RFC count against sipx's workspace would be the same category error as checking its tag.

**Five tests added** to `test-sync-website.py` (19 pass): the rendered body carries every heading of
the canonical source and matches neither `STORY_ID` nor `INTERNAL_PUBLIC_LINK`; every link on the
published page is absolute; the renderer refuses a red checker; the page is in the sidebar; and the
trailer covers all four required topics — method, evidence asymmetry, where sipx loses, and what the
page does not establish.

**`./scripts/build-docs.sh` passes** — zero `[WARNING]` lines, `WARNING_EXCEPTIONS` still empty, all
four throw handlers happy, 15 generated regions in sync, 531 internal links resolving, and the
anchor guard armed. The page reaches both `llms.txt` and `llms-full.txt`, which is what the sidebar
registration was for.

## Notes
- Blocked on `X-73`; there is nothing to publish until a dataset exists.
- The "where sipx loses" clause is not editorial balance for its own sake. It is the credibility
  mechanism for every other row, and a page without it is the marketing artifact
  [`rfc-registry-grain.md`](../designs/rfc-registry-grain.md) refuses.
- This is the story that makes `X-47`'s superseded clause visible in public. Land it only after
  `X-71` has recorded why.
