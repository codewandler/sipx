---
id: {{ID}}
title: {{TITLE}}
pillar: {{PILLAR}}
status: backlog
priority:
design:
epic:
areas:
predicate:
announcement:
note:
---

# {{TITLE}}

## Goal
One or two sentences: the outcome this delivers and which value/pillar it serves.

## Acceptance
- [ ] A testable criterion. A behavioral change names the failing-first test that proves it.
- [ ] …

## Progress
- (running log / checklist — a resuming agent reads this to know exactly where things stand)

## Notes
- Links, blockers, design pointers, relevant files.
- If this story bears on an alpha predicate (`docs/roadmap.md`), set `predicate:` in the frontmatter —
  `3`, or `[3, 7]` for one that bears on two. That field is the **only** place the association is
  recorded, and `docs/maturity.md` reports the predicate open until every story declaring it is `done`.
- If it bears on a beta-announcement predicate, use `announcement:` the same way. The roadmap defines
  those five predicates; the report treats an invalid number as an error and an undeclared computed
  predicate as unknown.
