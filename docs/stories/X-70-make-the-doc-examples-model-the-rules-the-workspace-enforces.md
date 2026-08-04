---
id: X-70
title: Make the doc examples model the rules the workspace enforces
pillar: Build
status: done
priority: 12
design: docs/designs/docs-depth.md
epic: docs-depth
areas: [sipx-call, sipx-ua]
predicate:
announcement: 5
note: the canonical dispatcher example teaches a detached spawn and an expect, both banned in library code · beta-2
---

# Make the doc examples model the rules the workspace enforces

## Goal

Stop the published examples from teaching patterns the workspace forbids, so the first code a user
copies is code that would pass this project's own review.

## Acceptance

- [x] The dispatcher doc example at `crates/sipx-call/src/dispatch.rs:15-31` no longer detaches a
      `tokio::spawn` with an ignored `JoinHandle`, and no longer calls `.expect()` on a fallible
      value. It shows the bounded, cancellation-aware shutdown the workspace actually requires
      (`AGENTS.md` non-negotiable 5) and propagates its error.
- [x] Every remaining doc-comment example across the workspace is audited against non-negotiables 3
      and 5 — no `unwrap`, `expect`, `panic` or raw indexing outside a context where the example is
      demonstrating the failure itself, no unbounded or detached background work. The audit's result
      is recorded in Progress, including examples found clean.
- [x] The four samples inlined into the site from `crates/*/examples/` are covered by the same audit,
      and `sync-website.py --check` still passes byte-exactly afterwards.
- [x] Where an example genuinely needs to be short at the cost of rigor, it says so in one line of
      prose in the example itself rather than silently modelling the anti-pattern.
- [x] `./scripts/gate.py` green.

## Acceptance note on the predicate

This story declares `announcement: 5`. The adoption surface is not current while the canonical
example contradicts the contract the crate documentation states two screens above it — that is an
honesty defect in published material, not a style preference.

## Progress

- The first `sync-website.py --check` after adding the executable-doc guard failed on exactly the
  two known dispatcher lines: panic-prone `.expect()` and a detached `tokio::spawn`. The replacement
  parses with `?`, caps concurrent calls at 64, refuses overflow with 503, reaps completed work
  through a `JoinSet`, propagates both task and call errors, and aborts/joins remaining calls on
  shutdown. Its `sipx-call` doctest compiles with all features.
- Audited all 25 fenced doc-comment blocks. The executable Rust blocks in
  `sipx-app-protocol/src/lib.rs`, `sipx-sip/src/build.rs` and `sipx-sip/src/headers/mod.rs` were
  clean; the dispatcher block was the only live anti-pattern. Sixteen text, four ABNF and one shell
  block contain no executable panic or detached-work pattern. The text block in
  `sipx-cli/tests/recording_bounds.rs` deliberately quotes a removed `unwrap_or_default` defect and
  explains it immediately below.
- Audited the four site-inline examples: `answer_a_call`, `place_a_call`, `parse_a_message` and
  `register` have no direct `unwrap`, `expect`, `panic`, raw indexing or detached work. Their waits
  are foreground or explicitly bounded. The generalized public-doc guard now rejects those patterns
  in future fenced Rust doc examples, and all 14 generated website regions remain byte-exact.

## Notes
- Found by the 2026-08-04 capability review, which noted the irony directly: 19 non-test
  `unwrap`/`expect` sites in ~95k lines of source, and one of them is in the snippet users copy first.
- Small and mechanical. It is beta-2 because it is cheap and because it is published.
