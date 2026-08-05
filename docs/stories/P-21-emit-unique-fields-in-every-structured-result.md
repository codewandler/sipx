---
id: P-21
title: "Emit unique fields in every structured result"
pillar: "Phone"
status: ready
priority: 3
epic: diagnostic-automation
areas: [sipx-cli]
design: docs/designs/diagnostic-automation.md
note: "external review finding 14 · browser-audio emits media_profile twice"
---

# Emit unique fields in every structured result

## Goal

Make duplicate field names unrepresentable or explicitly rejected in CLI reports, while preserving
the deterministic field order and text/JSON fact parity scripts already rely on.

## Acceptance

- [ ] A failing-first browser-profile result test tokenizes object members without collapsing them
      into a map and proves `media_profile` is currently emitted twice.
- [ ] The report builder detects a duplicate at construction/insertion time or stores fields in an
      order-preserving unique representation; silently emitting duplicate JSON keys is impossible.
- [ ] Requested and negotiated media report composition assigns each field one owner.
      `media_profile` and every other common fact appear exactly once in both dial and answer.
- [ ] A repository test runs or renders every versioned CLI result producer and rejects duplicate
      object members recursively rather than relying on a JSON map parser's last-value behavior.
- [ ] Text output retains deterministic order and contains the same facts. Existing JSON field
      spelling and values are unchanged except for removal of the duplicate member.
- [ ] The generated CLI JSON-contract table remains synchronized, strict duplicate-rejecting
      consumers accept all fixtures, and the complete repository gate is green.

## Review evidence

Finding 14 observed two `media_profile` members in one successful structured browser-audio call;
ordinary parsers hid the defect by retaining one value.
