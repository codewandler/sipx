---
id: X-73
title: Derive the first comparison dataset with a repo-local skill
pillar: Build
status: done
priority: 16
design: docs/designs/stack-comparison.md
epic: stack-comparison
areas: [docs, scripts]
predicate:
announcement:
note: the repo's first tracked .claude/ file · a refresh must be a command, not a conversation · blocked on X-72
---

# Derive the first comparison dataset with a repo-local skill

## Goal

Turn the derivation process from a conversation into a repeatable procedure, and use it to produce
the first external subject's observations.

## Acceptance

- [ ] `.claude/skills/compare-stacks/SKILL.md` exists with frontmatter `name` matching the
      directory and a `description`. It is the repository's first tracked file under `.claude/`;
      `.gitignore` already reserves that space and **must not need changing**.
- [ ] The skill documents the whole procedure: scope from `stacks.json` and `dimensions.json` ·
      clone and pin each subject at a tag · fan out parallel agents by dimension group (capability
      inventory, testing and CI, security posture and advisories, documentation, maturity) ·
      apply the evidence rules · emit `observations/<stack>.json` · iterate against
      `comparison-report.py --check` until clean.
- [ ] It states the evidence rules as rules, not advice: the confidence ladder, at least one
      falsifiable evidence entry, pinned version and date, a `reproduce` command for anything
      claimed `measured`, and **never write sipx's generated cells by hand**.
- [ ] It requires the run to record evidence asymmetry — which subjects were read at source level
      and which only from published material — because that asymmetry flatters sipx and the page
      has to say so.
- [ ] `references/dimensions.md` carries the per-dimension derivation recipe: what to look for and
      what counts as evidence at each tier. This is the part that rots if it stays in an agent's
      head.
- [ ] Observations exist for at least one external subject, produced **by running the skill**, and
      `./scripts/comparison-report.py --check` is clean over them.
- [ ] Every `measured` observation's `reproduce` command has been executed and its output recorded
      in the story's Progress. A `reproduce` nobody ran is a citation that cannot fail.
- [ ] The skill is subject to the provenance check like any other tracked file, and passes under
      `X-71`'s scope — i.e. it names subjects only where the boundary permits.
- [ ] `./scripts/gate.py` green.

## Progress

Implemented 2026-08-04. Everything in Acceptance is satisfied.

- **`.claude/skills/compare-stacks/SKILL.md`** and **`references/dimensions.md`** are the
  repository's first tracked files under `.claude/`. Verified mechanically that no `.gitignore`
  edit was needed: the only `.claude` rule is scoped to `/.claude/worktrees/`, and
  `git check-ignore -v .claude/skills/compare-stacks/SKILL.md` exits 1.
- **The skill names no subject at all.** `.claude/` is not inside `COMPARISON_SCOPE`, so the
  procedure reads the subject list from `stacks.json` and says so as its first rule. The same
  constraint holds for `comparison-report.py`, and `test-comparison-report.py` asserts it
  structurally — no stack id from the dataset may appear in the checker's source.
- **`references/dimensions.md`** carries a recipe per dimension: what to look for, what counts as
  evidence at each tier, the `measured` command shape, and the trap specific to that row — for
  media, claiming a codec from a payload-type constant rather than from an encoder and a decoder;
  for security posture, reading a raw advisory count as a scoreboard; for testing, missing that a
  torture corpus is present but half disabled.

**The subject set is seven, all at `measured` tier**, decided here rather than in the design as the
design left open. More subjects make a better page and multiply the staleness burden; seven was
chosen to cover the languages a chooser actually weighs — C, C++, Go, C#, Rust and Java — and each
one is pinned at a tag with a command behind every finding.

**No Zig subject exists.** A search turned up no Zig SIP stack, only advice to bind to one of the C
libraries. A stack with no implementation cannot hold evidenced observations, so that is stated in
the published page's prose rather than carried as an empty row.

**Every `reproduce` command was executed as written.** A runner extracted all 42 from the dataset
and ran each in a fresh clone: **42 commands, 0 failed.** Spot-checked outputs against the
summaries they back — the transport module list against the transport enum for the C subject whose
enum names two transports it has no module for; 217 test translation units and 270 torture
assertions for the C++ subject; the 2003 / 2011 / 2024 first-tag, last-tag and last-commit triple
for the Java subject, which is the evidence behind its dormancy finding.

**One finding changed sipx's own row.** The maturity cell claimed sipx was younger than every
subject; measuring showed one subject's first tag predates sipx's entire repository history, so the
claim was rewritten to the one the evidence supports — repository history in weeks against two
lineages shipping since 2004 and 2005 — with no typed numbers, since a number about sipx that no
rule computes is exactly what this epic forbids.

`./scripts/comparison-report.py --check` reports **8 stacks over 6 dimensions, every claim
evidenced, none stale**. `check-provenance.sh` and `--history` both clean.

## Notes
- Blocked on `X-72`. The skill is only useful once the schema and checker define what it must emit.
- Whether the first subject set is one stack or several is decided here rather than in the design:
  more subjects make a better page and multiply the staleness burden, and that trade is better made
  with one dataset in hand.
- Prior research for the first subject already exists outside the repository and can seed the run,
  but every observation still needs its own evidence, version and date — inherited prose is not
  evidence.
