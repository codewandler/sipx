---
id: X-133
title: The JSON contract reader accepts rows that do not render
pillar: Quality
status: backlog
priority: 6
design:
epic:
areas: [docs, sipx-cli]
predicate:
announcement:
note: X-132 narrowed the flag half of check-cli-reference to real tables; `_document_json_contracts` in the same file still counts any three-celled `|` line between the region markers
---

# The JSON contract reader accepts rows that do not render

## Goal

Hold the versioned-contract half of `check-cli-reference.py` to reading *table rows*, the way
`X-132` did for the flag half, so a contract row spliced under prose cannot satisfy the documented
side of the comparison while rendering to a reader as literal pipes.

## Why

`X-132` fixed `document_command_flags` and left `_document_json_contracts` — in the same file,
against the same page — reading every line between `<!-- BEGIN cli-json-contracts -->` and its END
marker that starts with `|`, has three cells and does not contain `---`. Whether those lines form a
table is never asked, which is precisely the shape `X-132` was filed for.

Measured on the repaired page while implementing `X-132`: splicing a paragraph directly above the
last row of the contract table leaves the reader finding all five contracts, unchanged, though a
Markdown reader now meets that row as pipes at the end of a paragraph. The failure it would hide is
the one the region exists to catch — a versioned contract whose public row stopped rendering, while
`json_drift` reports agreement.

The remedy is now cheap: `X-132` left `table_rows` beside it as a module-level helper, so this is a
reuse rather than a second implementation. Note the existing `"---" in line` guard is doing the
delimiter row's job by hand and can go with it — it also silently drops any legitimate row whose
cells happen to contain a triple dash.

## Acceptance

- [ ] A failing-first fixture: a contract row written under prose inside the region reports that
      contract as undocumented rather than counting it.
- [ ] `_document_json_contracts` reads its rows through `table_rows`, and the hand-rolled `"---"`
      skip is gone with it.
- [ ] The over-narrowing property is pinned the way `X-131` and `X-132` pinned theirs: a reader
      that saw no rows reports every discovered contract as undocumented rather than going quiet.
- [ ] `./scripts/gate.py` green.

## Notes

Found while implementing `X-132`, and reported rather than folded into it: `X-132`'s Acceptance
names `document_command_flags`, and widening a narrowing diff to a second reader would have put two
behaviour changes behind one failing-first test.
