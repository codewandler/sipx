---
id: X-150
title: Normalize generic parameter names in API baselines
pillar: Build
status: backlog
priority: 45
design: docs/specs/api-compatibility.md
epic: api-stability
areas: [api, release]
predicate:
announcement:
note: conservative X-149 false red · parameter renames are positional and source-compatible
---

# Normalize generic parameter names in API baselines

## Goal

Keep the Supported-API guard strict about generic structure without rejecting a change that only
renames a type, lifetime or const parameter.

## Acceptance

- [ ] Canonicalization assigns positional identities to declared type, lifetime and const generic
      parameters and rewrites every reference consistently across signatures, bounds and where
      predicates.
- [ ] Renaming only generic parameters is compatible, while changing their order, kind, default,
      bound or use site remains breaking.
- [ ] Mutation vectors cover type, lifetime and const parameter renames plus one genuinely changed
      bound, and the current v1 baseline still checks without manual editing.
- [ ] Focused compatibility tests and the complete repository gate are green.

## Notes

- This is a conservative false positive only: the current checker may block a compatible rename,
  but it cannot let a breaking change through. It does not block the v1 content gate.
