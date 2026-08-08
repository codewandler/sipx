---
id: X-117
title: Make the generated reports discoverable
pillar: Build
status: ready
priority: 29
design:
epic: conformance
areas: [docs, website]
predicate:
announcement:
note: coverage, comparison, compliance and maturity are all reachable only by knowing their path
---

# Make the generated reports discoverable

## Goal

Give the generated reports a way in. `docs/coverage.md`, `docs/comparison.md`, `docs/compliance.md`
and `docs/maturity.md` are the project's evidence surface and each is reachable only by already
knowing its filename.

## Acceptance

- [ ] Every generated report is linked from a page a reader arrives at, and the link set is derived
      from what exists rather than hand-maintained.
- [ ] A failing-first check fails when a new generated report is added without becoming reachable.
- [ ] Each link states what the report measures and what it deliberately does not, so a reader does
      not have to open it to learn it is not a quality claim.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `X-66`'s adjacent findings, which noted the new coverage page is
  "consistent, and consistently undiscoverable" with its three siblings.
