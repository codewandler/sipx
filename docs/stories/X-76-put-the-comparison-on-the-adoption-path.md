---
id: X-76
title: Put the comparison on the adoption path
pillar: Build
status: done
priority: 18
design: docs/designs/stack-comparison.md
epic: stack-comparison
areas: [website, docs]
predicate:
announcement:
note: the page is in the sidebar and in nobody's prose · a sidebar entry is not a path to a page
---

# Put the comparison on the adoption path

## Goal

Make the comparison reachable from the pages a chooser actually reads, and make being unreachable a
test failure rather than something somebody has to notice.

## Acceptance

- [ ] `website/docs/guides/does-this-fit.md` points at the comparison from `## Choose something else
      when you need` — the section that lists eight categories and names no alternative — and from
      the link paragraph closing `## Security boundary`, which already routes readers to Security and
      RFC compliance.
- [ ] `website/docs/intro.md` lists it beside "Does sipx fit?" in the closing link list.
- [ ] `README.md` lists it beside the RFC compliance entry, using the absolute
      `https://codewandler.github.io/sipx/docs/reference/comparison` form its neighbours use.
- [ ] `website/docusaurus.config.js` carries it in the footer's **Project** column.
- [ ] **No comparison subject is named on any of these pages.** `COMPARISON_SCOPE` is three paths and
      none of them is here; every edit describes what the page answers, never who it answers it
      against. `./scripts/check-provenance.sh` clean.
- [ ] A failing-first test in `scripts/test-sync-website.py` requires at least one page in
      `CURRENT_SURFACE_PAGES` to link to `reference/comparison`, and it has been seen to fail with a
      link removed.
- [ ] `./scripts/build-docs.sh` passes — the four throw handlers see the new routes — and
      `./scripts/gate.py` is green.

## Progress

Implemented 2026-08-04. Everything in Acceptance is satisfied.

- **The test was written first and seen to fail.**
  `test_the_comparison_page_is_reachable_from_the_adoption_path` walks `CURRENT_SURFACE_PAGES`,
  skips the comparison page itself — a page does not reach itself — and requires at least one
  other to link to it. On the tree as `X-74` left it: `AssertionError: [] is not true : no
  current-surface page links to the comparison; a sidebar entry is not a path to a page`.
- **Four links added**, none naming a subject:
  - `does-this-fit.md` — a paragraph opening `## Choose something else when you need`, which frames
    the eight boundaries as boundaries rather than judgements and sends the reader on; and the
    existing link sentence closing `## Security boundary`, now Security · RFC compliance ·
    comparison;
  - `intro.md`, beside "Does sipx fit?" in the closing list;
  - `README.md`, beside the RFC compliance entry, in the absolute form its neighbours use;
  - the footer's **Project** column in `docusaurus.config.js`.
- **Three pages now reach it** — `README.md`, `website/docs/intro.md`,
  `website/docs/guides/does-this-fit.md` — and the test passes (20 tests in
  `test-sync-website.py`).
- `check-provenance.sh` and `--history` clean: the four pages describe what the comparison answers,
  never who it answers it against. `check-docs-links.py` resolves 531 relative links over 290
  internal pages, `rfc-report.py --check` still green.

## Notes
- The defect this closes was invisible to every existing check: the page is in `sidebars.js`, so it
  reaches the site nav, `llms.txt` and `llms-full.txt`, and no prose anywhere links to it. `X-47`
  made the public docs an adoption path and this page was published beside it rather than onto it.
- `README.md` and `does-this-fit.md` carry `ADOPTION_REQUIREMENTS` and `COUNTED_IN` obligations. A
  link is inert to both, but `rfc-report.py --check` is cheap and worth re-running.
