---
id: M-97
title: Make the extensibility guard tell the attribute from prose about it
pillar: Media
status: backlog
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

- [ ] The marked test reads `#[non_exhaustive]` as an attribute line of the preamble, not as a
      substring of it, for both the enum rule and the struct rule.
- [ ] A failing-first proof: a fixture whose doc comment names the attribute and whose declaration
      does not is reported, and fails before the fix.
- [ ] `ProviderKind` is resolved on its merits — its argument is `COMPLETE_REASON`'s sibling for
      enums, so it wants `EXHAUSTIVE_REASON` and the existing prose reads as one already.
- [ ] The same question asked of `UNBUILT_REASON`, `COMPLETE_REASON` and `EXHAUSTIVE_REASON`: those
      are `///` phrases and so are genuinely prose, but a doc comment quoting one of them over a
      *different* type would classify this one. Decide whether that is worth closing here or is a
      separate finding.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-09: filed from `M-83`, which hit the defect three times in one diff and only noticed
  because it was reconciling a 61/39 split against a list of 100.
