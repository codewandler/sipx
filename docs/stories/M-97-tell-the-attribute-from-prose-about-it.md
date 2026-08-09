---
id: M-97
title: Make the extensibility guard tell the attribute from prose about it
pillar: Media
status: done
priority: 8
design:
epic: media
areas: [scripts, sipx-media]
note: `preamble` is searched as one string, so a doc comment containing `#[non_exhaustive]` satisfies the rule; `sipx-media`'s `ProviderKind` passes on prose alone today
---

# Make the extensibility guard tell the attribute from prose about it

## Goal

`enum_problems` and `struct_problems` decide whether a type is marked by testing
`"#[non_exhaustive]" in above`, where `above` is [`preamble`]'s whole block — attributes *and* doc
comment, as one string. A `///` line that mentions the attribute therefore satisfies the rule
without the type carrying it. Make the check read the attribute as an attribute.

## Why

This is not hypothetical and it is not one type's mistake. `M-83` wrote three arguments that
explained a decision by naming the attribute — "`OverloadAlgorithm` is `#[non_exhaustive]` because
its token set is a registry", and two more — and all three were silently reclassified from *argued*
to *marked* by a guard that cannot tell the difference. They were caught by counting the split
against the failing-first list, which nothing in the gate does.

One type in the workspace passes on prose alone today:

    crates/sipx-media/src/speech/descriptor.rs:287 `ProviderKind`

It carries `/// Closed by design, and deliberately not `#[non_exhaustive]`: …`, which is a real and
well-made argument — for exactly the position the rule has a phrase for. It is not marked, it does
not say `EXHAUSTIVE_REASON`, and the guard has counted it as marked since `M-74`.
`sipx-app-protocol`'s `Output` has the same shape and is out of scope under `A-9`, so it stays as
it is.

The failure is quiet in the direction that matters. A type that argues its way past the rule in
prose is a type nobody reviewed under the rule, and the whole design of this check — the phrase at
the type, no suppression list — is built on the argument being written in the one form a reader can
grep for.

## Acceptance

- [x] The marked test reads `#[non_exhaustive]` as an attribute line of the preamble, not as a
      substring of it, for both the enum rule and the struct rule.
- [x] A failing-first proof: a fixture whose doc comment names the attribute and whose declaration
      does not is reported, and fails before the fix.
- [x] `ProviderKind` is resolved on its merits — its argument is `COMPLETE_REASON`'s sibling for
      enums, so it wants `EXHAUSTIVE_REASON` and the existing prose reads as one already.
- [x] The same question asked of `UNBUILT_REASON`, `COMPLETE_REASON` and `EXHAUSTIVE_REASON`: those
      are `///` phrases and so are genuinely prose, but a doc comment quoting one of them over a
      *different* type would classify this one. Decide whether that is worth closing here or is a
      separate finding.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-09: filed from `M-83`, which hit the defect three times in one diff and only noticed
  because it was reconciling a 61/39 split against a list of 100.
- 2026-08-09: implemented on `impl/M-97`. `marked` reads `_MARKED` — `#[non_exhaustive]` as its own
  line of the preamble, at any indentation — and `argued` requires a rationale phrase to *open* a
  doc line; `enum_problems`, `struct_problems` and `outstanding_structs` all go through them, so the
  rule and the debt line agree about what a guard is.

  Failing-first, at `6b6d32c` (`git merge-base main HEAD`), before the fix:

      $ python3 -m unittest scripts/test-audio-claims.py -k TheAttributeAndProseAboutIt
      FAIL: test_a_doc_comment_naming_the_attribute_does_not_guard_an_enum
        AssertionError: 1 != 0
      FAIL: test_a_doc_comment_naming_the_attribute_does_not_guard_a_struct
        AssertionError: 1 != 0
      FAIL: test_a_quoted_reason_phrase_does_not_argue_a_type_out
        AssertionError: 1 != 0
      ERROR: test_an_indented_attribute_is_still_the_attribute
        AttributeError: module 'check_audio_claims' has no attribute 'marked'
      Ran 7 tests — FAILED (failures=3, errors=3)

  The three failures are the guard accepting a preamble that only *names* the attribute; the three
  errors are the helper not existing yet. `test_the_attribute_itself_still_guards_both_rules` passed
  at the base, which is the point — the attribute always worked, only prose about it was the defect.

  **What moved, over the whole tree.** 281 reachable enums in the 11 guarded crates and 35 breakable
  structs in `MEDIA_SURFACE`. Enums went marked 219 / argued 61 / *marked on prose alone* 1 → marked
  219 / argued 62 / prose 0. Structs did not move: 25 marked, 10 argued, none on prose. The one enum
  is `ProviderKind` (`crates/sipx-media/src/speech/descriptor.rs`), which had counted as marked since
  `M-74` on a sentence saying it is deliberately **not** marked. Resolved as this story reads it: the
  argument was already `EXHAUSTIVE_REASON`'s in everything but spelling, so it is now written as
  `/// Exhaustive by design: this specification defines two substitutable contracts, and a third
  would be a different document …`. Nothing about the type changed; only the form of its argument.

  One further move, in a crate the rule does not hold: `sipx-app-protocol`'s `Output`
  (`interpreter.rs:214`) also passed on prose. `A-9` leaves that crate out, so — as this story says —
  it stays as it is, and the summary line's stated-exclusion count went **18 → 19**. That number is
  the movement being visible rather than silent, which is the whole point of the defect.

  **Acceptance row 4, decided: closed here, not filed.** The phrases are prose, but requiring one to
  open a doc line costs a single anchored regex and closes the *quoting* case the story names — a
  sentence that quotes `` `/// Exhaustive by design:` `` while explaining another type. Measured
  before the change: anchoring moves **zero** types in the tree, so it narrows nothing that was
  honestly argued. The residue is not closable by any rule — a phrase written at the start of a line
  about somebody else's type reads identically to one about this type — so it is stated in `argued`'s
  docstring as a reviewer's question rather than filed as a story nobody could finish.

  Verified: `python3 -m unittest scripts/test-audio-claims.py` (117 tests, OK),
  `./scripts/check-audio-claims.py --check` (exit 0),
  `cargo check --workspace --all-features --all-targets` (exit 0), `cargo fmt --all` (no change).
  `./scripts/gate.py` deliberately not run here; the coordinator runs one gate per wave.

  Owed to `CHANGELOG.md`, which this story may not edit: *The extensibility guard now reads
  `#[non_exhaustive]` as an attribute rather than as a substring of the text above a type, so a doc
  comment naming the attribute no longer stands in for carrying it; `sipx-media`'s `ProviderKind`
  states its argument as `Exhaustive by design:`.*

- 2026-08-09: closed at the `1.0.0-rc.15` boundary, against the wave gate run on this tree.
