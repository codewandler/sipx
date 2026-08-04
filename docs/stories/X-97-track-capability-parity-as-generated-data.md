---
id: X-97
title: Track capability parity as generated data
pillar: Build
status: in-progress
priority: 1
design: docs/designs/stack-comparison.md
epic: stack-comparison
areas: [docs, scripts, comparison, m13, parity-wave-1]
predicate:
announcement:
note: M13 discovery gate · every public capability gets evidence, ownership and a disposition
---

# Track capability parity as generated data

## Goal

Turn the pinned comparison from six chooser-facing dimensions into a leaf-level capability ledger
that can prove the selected endpoint target is complete and can identify work owned elsewhere.

## Acceptance

- [ ] The comparison schema represents a stable capability key, evidence, confidence, ownership,
      disposition and optional open story for every public capability in the pinned subject release.
- [ ] Ownership is exactly one of sipx, the cluster repository, not shipped by the subject, or not
      applicable under sipx's vision; every non-applicable row carries a rationale.
- [ ] A checker rejects duplicate, unowned, unevidenced and stale rows, and rejects an open sipx row
      without a story link.
- [ ] The first inventory covers exported endpoint APIs, transports, methods, authentication,
      lifecycle, media, examples and operational surfaces rather than inferring them from README
      headings.
- [ ] Cluster-owned rows link to an existing cluster story or cause one to be filed there before M13
      can close. No proxy, registrar, routing or deployment implementation is copied into this repo.
- [ ] Subject-specific identity and evidence remain confined to the comparison data directory and
      generated comparison page; stories, generic designs, source and commit text remain neutral.
- [ ] A fixture mutation proves each new checker refusal, and `./scripts/gate.py` is green.

## Progress

- In progress. The first pinned leaf-level inventory and its checker are being added on the
  capability-ledger feature branch. Any newly discovered sipx-owned gap joins M13 before the load
  milestone may start.
