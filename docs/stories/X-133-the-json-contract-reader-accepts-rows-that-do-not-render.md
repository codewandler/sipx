---
id: X-133
title: The JSON contract reader accepts rows that do not render
pillar: Quality
status: done
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

- [x] A failing-first fixture: a contract row written under prose inside the region reports that
      contract as undocumented rather than counting it.
- [x] `_document_json_contracts` reads its rows through `table_rows`, and the hand-rolled `"---"`
      skip is gone with it.
- [x] The over-narrowing property is pinned the way `X-131` and `X-132` pinned theirs: a reader
      that saw no rows reports every discovered contract as undocumented rather than going quiet.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-09: implemented on `impl/M-97`, as the second commit of a two-defect branch. The change is
  the one-line reuse the story described — `for line in table_rows(body)` in place of the
  `startswith("|") and "---" not in line` scan — plus the docstring that records why.

  Failing-first, at `6b6d32c` (`git merge-base main HEAD`), before the fix:

      $ python3 -m unittest scripts/test-cli-reference.py -k TheContractTableReader
      FAIL: test_a_row_spliced_under_prose_is_not_a_documented_contract
        AssertionError: Lists differ:
          ['versioned CLI contract `sipx.other.v1` is not documented'] != []
      FAIL: test_a_header_and_delimiter_under_prose_do_not_open_a_table_either
        AssertionError: Lists differ:
          ['versioned CLI contract `sipx.other.v1` is not documented'] != []
      FAIL: test_a_row_whose_cells_carry_a_triple_dash_is_still_a_row
        AssertionError: Lists differ:
          [] != ['versioned CLI contract `sipx.other.v1` is not documented']
      Ran 6 tests — FAILED (failures=3)

  The first two are the defect: a row spliced under prose was counted, so `json_drift` reported
  agreement about a row that renders as pipes. The third is the cost of the hand-rolled `---` skip,
  which dropped a legitimate row whose cells carried a triple dash — that row now reads.

  Over-narrowing is loud, pinned two ways as `X-131` and `X-132` pinned theirs.
  `test_a_reader_that_saw_no_rows_would_be_reported_not_silent` puts a region with no table in front
  of the reader and asserts both discovered contracts come back as *not documented* rather than
  silence, and `test_no_contract_row_of_the_public_page_sits_outside_a_table` holds the live
  `website/docs/reference/cli.md` region against the wide reader this replaced — where the two agree,
  every `|` line between the markers is a row of a real table. Both pass; the live page needed no
  repair.

  Verified: `python3 scripts/test-cli-reference.py` (31 tests, OK) and
  `./scripts/check-cli-reference.py` (exit 0: *8 command helps and 5 versioned JSON contracts
  agree*). No Rust changed in this commit. `./scripts/gate.py` deliberately not run here; the
  coordinator runs one gate per wave.

  Owed to `CHANGELOG.md`, which this story may not edit: *`check-cli-reference.py` reads the
  versioned-contract table through the same row reader as the flag table, so a contract row spliced
  under prose is reported as undocumented instead of counted.*

## Notes

Found while implementing `X-132`, and reported rather than folded into it: `X-132`'s Acceptance
names `document_command_flags`, and widening a narrowing diff to a second reader would have put two
behaviour changes behind one failing-first test.

- 2026-08-09: closed at the `1.0.0-rc.15` boundary, against the wave gate run on this tree.
