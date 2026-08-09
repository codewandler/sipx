---
id: X-132
title: The CLI reference reader accepts rows that do not render
pillar: Quality
status: backlog
priority: 5
design:
epic:
areas: [docs, sipx-cli]
predicate:
announcement:
note: check-cli-reference counts any `|`-leading line in a section as a documented flag, and load-responder's last six flags sit below a paragraph with no delimiter row — documented to the checker, prose to a reader
---

# The CLI reference reader accepts rows that do not render

## Goal

Make `check-cli-reference.py`'s page half hold a command's flag rows to being *a table*, so a row
that a Markdown reader will render as literal text cannot satisfy the documented half of the
comparison — and put `sipx load-responder`'s six affected flags back into a table on the public
page.

## Why

`document_command_flags` reads every line of a command's section that starts with `|` and takes a
`` `--flag` `` out of its first cell. Whether those lines form a table is never asked. GFM tables
cannot interrupt a paragraph and need a delimiter row, so a run of `|` lines that resumes after
prose is a paragraph continuation — pipes and all — on the rendered page.

That is the state of `website/docs/reference/cli.md` today. The `sipx load-responder` section opens
a table at line 371, runs six rows, and at line 380 splices in the **Sizing `--max-active` against
a generator** paragraph. Line 390 resumes with `| `--answer-percent <P>` | … |` and no blank line
and no delimiter row, and five more rows follow it:

```
| `--answer-percent <P>` | `--reject-status <CODE>` | `--dialog-duration <S>`
| `--mode <M>`           | `--local <ADDR>`         | `--transport <T>`
```

Six flags the checker counts as documented, and that an operator reading the page meets as a wall
of pipes at the end of a paragraph about sizing. The checker cannot see the difference because it
never asks what the lines belong to, which is the same shape of defect as `X-131` on the other
half: a reader that matches a token wherever it appears rather than reading the structure it
appears in.

Found while implementing `X-131`, which narrowed the *executable* half of the same comparison and
so had this section open. Reported rather than fixed, because moving prose on the public page is a
different change from narrowing a checker and belongs in its own diff.

## Acceptance

- [ ] A failing-first fixture: a command section whose rows resume after a paragraph, with no
      delimiter row, reports those flags as absent from the page rather than counting them.
- [ ] `document_command_flags` reads a command's flags from the rows of a delimited table, so a
      `|` line that is not part of one is not a documented flag.
- [ ] `website/docs/reference/cli.md`'s `sipx load-responder` section carries all twelve of its
      flags in one table, with the sizing paragraph below it rather than through it.
- [ ] The rest of the page is checked for the same shape, since nothing has been asking.
- [ ] `./scripts/gate.py` green.

## Notes

Worth doing on the page even before the checker: the six flags below the splice include
`--transport`, `--local` and `--mode`, which is most of what a reader configuring a responder run
needs after the ceiling.

- 2026-08-09: **the instance is repaired; the blind spot is what remains.** The spliced table was
  the coordinator's, introduced while finishing `X-129` — the sizing paragraph went in between two
  rows, so six flags after it were documented to the checker and rendered as a wall of pipes to a
  reader. The paragraph now follows the whole table and the rows are one table again.
  That leaves this story exactly what it was filed for: `document_command_flags` takes a flag from
  any `|`-leading line without asking whether those lines form a table, so the next person to write
  prose into a flag list gets the same silent pass. Same shape as `X-131`, on the other half of the
  same comparison — and this time the reader that could not see it let a real defect into the
  published page for the length of one release.

