---
id: X-131
title: Help prose cannot name another command's flag
pillar: Quality
status: done
priority: 5
design:
epic:
areas: [sipx-cli, docs]
predicate:
announcement:
note: check-cli-reference reads --flag tokens out of a command's whole help text, so a cross-reference in prose reads as an undocumented option
---

# Help prose cannot name another command's flag

## Goal

Let one command's help text refer to another command's flag by name, without
`check-cli-reference.py` reporting that flag as an undocumented option of the command whose help
merely mentioned it.

## Acceptance

- [x] `check-cli-reference.py` derives a command's option set from the option entries of its help,
      not from every `--token` appearing anywhere in the text. `scripts/check-cli-reference.py:88`
      reads the options block's entries; `_OPTIONS_BLOCK` and `_OPTION_ENTRY` at
      `scripts/check-cli-reference.py:39`.
- [x] A test or fixture proves the narrowed reader still catches the case the wide one exists for:
      a real option present in the executable help and absent from the public page.
      `TheOptionReader.test_an_undocumented_option_is_still_a_failure` and its compact-layout twin,
      `scripts/test-cli-reference.py:207`; and
      `test_a_reader_that_saw_no_entries_would_be_reported_not_silent`, which pins the property that
      makes over-narrowing loud rather than quiet.
- [x] `sipx load-responder`'s `--max-active` guidance names the generator's flag as `--concurrency`
      again, since that is the spelling an operator types. `crates/sipx-cli/src/cli.rs:481`.
- [x] `./scripts/gate.py` green.

## Notes

Found while writing `X-129`, which needed the responder's ceiling guidance to point at the
generator's concurrency flag. The first draft said "give this ceiling headroom over the generator's
`--concurrency`" in the doc comment on `LoadResponderOptions::max_active`, and the checker reported:

```
cli reference: load-responder: executable option `--concurrency` is not documented
```

`help_options` in `scripts/check-cli-reference.py` matches `--[a-z][a-z0-9-]*` over the entire
`--help` output, so a flag named in a description becomes a flag of that command. `X-129` worked
around it by writing "the generator's concurrency" in prose and spelling the flag only on the
public page, which is a smaller thing to say than the sentence it wanted.

Not urgent, and not a defect in what the checker is *for*: it exists so an option cannot ship
undocumented, and it does that. Narrowing the reader has to keep that property, which is why the
second row above is on this story and not optional — a parse that reads only option entries is
easy to write and easy to make too permissive.

## Progress

- 2026-08-09: implemented on `impl/X-131`, alongside `M-96`.

  Failing-first, end to end and in the encounter's own words. The workaround was undone first —
  `LoadResponderOptions::max_active` now says "Give it headroom over the generator's
  `--concurrency` rather than matching it", mirroring the sentence already on the public page —
  and the unmodified checker was run against it at the merge base `d3f6324`:

  ```
  $ ./scripts/check-cli-reference.py
  cli reference: load-responder: executable option `--concurrency` is not documented
  EXIT=1
  ```

  `TheOptionReader` in `scripts/test-cli-reference.py` was written against the same shape and ran
  red five of eight at that base, reporting the identical sentence, plus
  `AssertionError: Items in the second set but not the first: '--concurrency'` from the direct
  `help_options` assertion.

  **How the reader now tells prose from an option.** It reads the entries of the help's `Options:`
  block instead of the whole text. An entry opens its own line — clap sets entries at column two,
  or six where an absent short flag is padded past, and indents everything belonging to an entry to
  ten — and an entry's line is the option's spellings and value placeholders, with any description
  beyond them set across a two-space column gap. Both of clap's layouts are covered: the long
  help's one-entry-per-line block, which is what the binary renders and where this was met, and the
  compact two-column block, where the description shares the entry's line and only the leading
  spelling list is read. A possible-value line (`- signalling: …`) is excluded twice over — by the
  indent, and because its dash is followed by a space rather than a spelling.

  **Why over-narrowing cannot go quiet**, which is what the second Acceptance row is guarding:
  `help_drift` compares in both directions, so a reader that stopped seeing entries would report
  every documented option as absent from the executable help rather than passing. That property is
  now pinned by `test_a_reader_that_saw_no_entries_would_be_reported_not_silent`, and the
  undocumented-option direction by two tests, one per layout.

  Measured rather than argued: run both readers over all eight real command helps, the wide one and
  the narrow one agree on every option of every command except the one this story is about —
  `answer` 23/23, `dial` 28/28, `load` 17/17, `peers` 13/13, `register` 21/21, `scenario` 13/13,
  `devices` 0/0, and `load-responder` 13 → 12, losing exactly `--concurrency`. Nothing else on the
  surface moved.

  After: `python3 scripts/test-cli-reference.py` 18 tests OK, and `./scripts/check-cli-reference.py`
  exits 0 with "8 command helps and 5 versioned JSON contracts agree".

  Filed while here: `X-132`. Narrowing the executable half of this comparison meant reading the
  page half beside it, and the page half has the same shape of defect — it counts any `|`-leading
  line in a command's section as a documented flag without asking whether those lines form a table.
  `sipx load-responder`'s last six flags sit below a paragraph spliced through the middle of its
  table, with no delimiter row, so they are documented to the checker and prose to a reader. Left
  alone here: moving text on the public page is a different change from narrowing a checker.

  **Owed CHANGELOG sentence** (fenced file, not edited here): *Fixed — one command's `--help` may
  name another command's flag; the CLI-reference check reads a command's options from its help's
  option entries rather than from every `--token` in the text. `sipx load-responder --help` sizes
  `--max-active` against the generator's `--concurrency` by name again.*

- 2026-08-09: closed at the `1.0.0-rc.14` boundary, against the wave gate run on this tree.
