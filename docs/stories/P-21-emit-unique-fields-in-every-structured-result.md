---
id: P-21
title: "Emit unique fields in every structured result"
pillar: "Phone"
status: in-progress
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

- [x] A failing-first browser-profile result test tokenizes object members without collapsing them
      into a map and proves `media_profile` is currently emitted twice.
- [x] The report builder detects a duplicate at construction/insertion time or stores fields in an
      order-preserving unique representation; silently emitting duplicate JSON keys is impossible.
- [x] Requested and negotiated media report composition assigns each field one owner.
      `media_profile` and every other common fact appear exactly once in both dial and answer.
- [x] A repository test runs or renders every versioned CLI result producer and rejects duplicate
      object members recursively rather than relying on a JSON map parser's last-value behavior.
- [x] Text output retains deterministic order and contains the same facts. Existing JSON field
      spelling and values are unchanged except for removal of the duplicate member.
- [ ] The generated CLI JSON-contract table remains synchronized, strict duplicate-rejecting
      consumers accept all fixtures, and the complete repository gate is green.

## Review evidence

Finding 14 observed two `media_profile` members in one successful structured browser-audio call;
ordinary parsers hid the defect by retaining one value.

## Progress

- `docs/specs/diagnostic-phone.md` section 6.3 defines ordered unique common reports, assigns
  requested and negotiated media fields to one owner, and requires recursive duplicate rejection
  plus inventory coverage for every versioned CLI producer. DPH-20 through DPH-22 carry the
  executable vectors. Board regeneration and the complete gate remain deferred to push.
- The browser-audio process proof first rejected the repeated raw `media_profile`, then passed for
  both roles after negotiated media stopped claiming the requested field. `Report` now has private
  order-preserving unique storage. A recursive decoder rejects repeated root, nested and array
  object members, and the executable CLI-reference inventory requires real process coverage for
  devices, load, load-responder readiness, load-responder summaries and scenario envelopes.
  Default/all-feature strict lints, 112 CLI unit tests, the five focused producer processes, the
  executable reference check, documentation links, fixed-sleep and provenance checks pass. The
  complete gate and board regeneration remain deferred to push.
