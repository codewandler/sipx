---
id: X-132
title: The CLI reference reader accepts rows that do not render
pillar: Quality
status: done
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

- [x] A failing-first fixture: a command section whose rows resume after a paragraph, with no
      delimiter row, reports those flags as absent from the page rather than counting them.
- [x] `document_command_flags` reads a command's flags from the rows of a delimited table, so a
      `|` line that is not part of one is not a documented flag.
- [x] `website/docs/reference/cli.md`'s `sipx load-responder` section carries all twelve of its
      flags in one table, with the sizing paragraph below it rather than through it.
- [x] The rest of the page is checked for the same shape, since nothing has been asking.
- [x] `./scripts/gate.py` green.

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

## Progress

- 2026-08-09: **the blind spot is closed.** `document_command_flags` now takes its flags from
  `table_rows`, which reads a header row, a delimiter row of dashes directly beneath it, and the
  body rows running to the first line that is not one. Both halves of that shape are enforced,
  because a table can neither omit its delimiter row nor interrupt a paragraph.

  *Failing-first, on the page itself.* Splicing a paragraph above `load-responder`'s
  `--transport` row — the exact shape that shipped — left the checker at the merge base
  (`2a91041`) reporting success:

  ```
  $ ./scripts/check-cli-reference.py --binary target/cli-reference/debug/sipx
  cli reference: 8 command helps and 5 versioned JSON contracts agree
  EXIT=0
  ```

  The same splice against the narrowed reader:

  ```
  cli reference: load-responder: executable option `--transport` is not documented
  EXIT=1
  ```

  Three of the new `TheFlagTableReader` rows failed at the base for the same reason — the spliced
  fixture produced zero problems, and `document_command_flags` returned `{'--transport',
  '--max-active'}` where only `--max-active` renders.

  *Over-narrowing is loud, and pinned.* `help_drift` compares both ways, so a page reader that
  stopped seeing rows reports every executable option as undocumented rather than going quiet;
  `test_a_reader_that_saw_no_rows_would_be_reported_not_silent` asserts both messages verbatim,
  as `X-131` did for the executable half.

  *The rest of the page.* All eight command sections were audited against the wide reader this
  story replaced, and the two agree — so no section carries a row outside a table.
  `test_no_command_section_of_the_public_page_carries_a_row_outside_a_table` keeps that true as the
  page changes. A sweep of every tracked Markdown file found no other page with the shape; the only
  loose `|` lines in the repository are the illustration inside this story's own fenced block.

  *Left open.* The `sipx load-responder` table itself needed no work — the coordinator repaired it
  on `main` in `18b4f38`, before this branch's base, and the twelve flags are one table at
  `website/docs/reference/cli.md:371`. `X-133` is filed for the same blind spot in
  `_document_json_contracts`, which reads the JSON-contract region of this same page and was
  measured to still count a row spliced under prose.

  CHANGELOG sentence owed at integration: *The CLI-reference checker now reads a command's
  documented flags from the rows of a Markdown table, so a flag list broken by prose is reported
  instead of silently accepted.*

- 2026-08-09: closed at the `1.0.0-rc.14` boundary, against the wave gate run on this tree.
