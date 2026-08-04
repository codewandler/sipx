---
id: X-65
title: Assert the branch and tag RNG is cryptographic
pillar: Build
status: backlog
priority: 13
design: docs/designs/input-hardening.md
epic: input-hardening
areas: [sipx-sip, sipx-transport]
predicate:
announcement:
note: spec says cryptographic because a guessable branch is a response-injection primitive · nothing fails if it stops being · beta-1
---

# Assert the branch and tag RNG is cryptographic

## Goal

Make the Via branch and tag generator's cryptographic property fail a test when it stops holding,
rather than depending on review of the call site.

## Acceptance

- [ ] A test asserts the generated Via branch carries the RFC 3261 §8.1.1.7 `z9hG4bK` magic cookie
      and the full documented entropy width, over a sample large enough that a truncated or
      counter-derived generator fails it.
- [ ] The property is pinned by construction as well as statistically: swapping the generator for a
      non-cryptographic source fails to compile or fails a test, and the story's Progress log records
      that demonstration.
- [ ] Dialog tags (RFC 3261 §19.3) are covered by the same assertions.
- [ ] The statistical bound states its arithmetic in a comment on the line — the chosen threshold and
      the resulting false-failure rate — so the test cannot become a retry.
- [ ] `./scripts/gate.py` green, including `check-fixed-sleep.py`.

## Progress
- (not started)

## Notes
- `docs/specs/sip-transport.md:110` states the requirement and the reason: a guessable branch lets an
  off-path attacker inject responses. The generator satisfies it today; nothing detects a change.
- Small story, deliberately separate from `X-64`: a different property, a different failure, and it
  should not ride green on another story's result.
- Do not weaken the assertion to make it fast. If a large sample is slow, sample in one test rather
  than spreading a weak assertion across several.
