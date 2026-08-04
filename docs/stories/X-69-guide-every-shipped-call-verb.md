---
id: X-69
title: Guide every shipped call verb
pillar: Build
status: backlog
priority: 17
design: docs/designs/docs-depth.md
epic: docs-depth
areas: [website, sipx-call]
predicate:
announcement:
note: hold, transfer, DTMF, playback, recording and coupling all ship and appear only as bullets · follow-up
---

# Guide every shipped call verb

## Goal

Make every call verb sipx ships findable from the guides, so shipped work stops being
indistinguishable from unbuilt work to a reader of the site.

## Acceptance

- [ ] A guide exists for each shipped verb not currently covered: hold and resume, blind transfer,
      attended transfer, sending and collecting DTMF, playback, recording, and two-leg coupling.
      Place, answer and register already have theirs.
- [ ] Each guide's sample is inlined by `sync-website.py` from a real file under `crates/*/examples/`,
      compiled by CI like the existing four. Where no example file exists, this story writes one.
- [ ] The three example files currently not surfaced on the site are either surfaced or, with a
      recorded reason, deliberately left internal.
- [ ] No sample is hand-written into Markdown. `sync-website.py --check` passes byte-exactly.
- [ ] `does-this-fit.md` links each claimed capability to its guide, so the fit list stops being the
      only place a feature is mentioned.
- [ ] `build-docs.sh` passes with no new `WARNING_EXCEPTIONS` entry; `./scripts/gate.py` green.

## Progress
- (not started)

## Notes
- Follow-up rather than beta-1: it is real competitive ground and a substantial content and example
  effort, and it does not make anything already published untrue.
- Watch the cost the design flags — every example added is compiled on every gate run by
  `cargo build --workspace --examples`. If the count becomes noticeable, say so in Progress rather
  than quietly dropping examples from the site.
- Pairs with `X-68`, which explains the layering; this one explains the verbs.
