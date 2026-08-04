---
id: X-70
title: Make the doc examples model the rules the workspace enforces
pillar: Build
status: backlog
priority: 12
design: docs/designs/docs-depth.md
epic: docs-depth
areas: [sipx-call, sipx-ua]
predicate:
announcement: 5
note: the canonical dispatcher example teaches a detached spawn and an expect, both banned in library code · beta-1
---

# Make the doc examples model the rules the workspace enforces

## Goal

Stop the published examples from teaching patterns the workspace forbids, so the first code a user
copies is code that would pass this project's own review.

## Acceptance

- [ ] The dispatcher doc example at `crates/sipx-call/src/dispatch.rs:15-31` no longer detaches a
      `tokio::spawn` with an ignored `JoinHandle`, and no longer calls `.expect()` on a fallible
      value. It shows the bounded, cancellation-aware shutdown the workspace actually requires
      (`AGENTS.md` non-negotiable 5) and propagates its error.
- [ ] Every remaining doc-comment example across the workspace is audited against non-negotiables 3
      and 5 — no `unwrap`, `expect`, `panic` or raw indexing outside a context where the example is
      demonstrating the failure itself, no unbounded or detached background work. The audit's result
      is recorded in Progress, including examples found clean.
- [ ] The four samples inlined into the site from `crates/*/examples/` are covered by the same audit,
      and `sync-website.py --check` still passes byte-exactly afterwards.
- [ ] Where an example genuinely needs to be short at the cost of rigor, it says so in one line of
      prose in the example itself rather than silently modelling the anti-pattern.
- [ ] `./scripts/gate.py` green.

## Acceptance note on the predicate

This story declares `announcement: 5`. The adoption surface is not current while the canonical
example contradicts the contract the crate documentation states two screens above it — that is an
honesty defect in published material, not a style preference.

## Progress
- (not started)

## Notes
- Found by the 2026-08-04 capability review, which noted the irony directly: 19 non-test
  `unwrap`/`expect` sites in ~95k lines of source, and one of them is in the snippet users copy first.
- Small and mechanical. It is beta-1 because it is cheap and because it is published.
