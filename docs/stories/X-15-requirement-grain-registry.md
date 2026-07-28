---
id: X-15
title: Consider requirement-grain rows in the RFC registry
pillar: Build
status: backlog
priority:
design:
epic: conformance
areas: [docs, build]
note: track: docs · an offer, not a dependency — decide whether per-RFC grain is enough
---

# Consider requirement-grain rows in the RFC registry

## Goal
Decide whether `docs/rfc/registry.toml` should be able to say something finer than one status per
RFC — and if so, adopt the requirement-grain extension a downstream project has designed against
the same schema.

## Acceptance
- [ ] A decision is recorded either way. "Per-RFC grain is the right measurement for a kernel" is
      a perfectly good answer and closes this story.
- [ ] If adopted: an entry may carry `[[rfc.requirement]]` rows — section, requirement reference,
      applicability, status, proving tests — and `rfc-report.py --check` verifies them the way it
      verifies the per-RFC claims today.
- [ ] If adopted: the four coverage kinds (syntax, behavioral, role, interop) are reported
      separately, because "parses it" and "behaves per it" are the distinction the current
      parse-only status already half-makes.
- [ ] Either way, the downstream registry can keep inheriting kernel rows by reference at a pinned
      version, so no claim is made twice.

## Progress
- Not started; nothing is blocked on it.

## Notes
- Offered by [sipx-clstr](https://github.com/codewandler/sipx-clstr), which is building an
  independent registry instance on this schema for a different role set (proxy, registrar) and has
  already decided to extend locally rather than wait
  ([ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md)). This story
  exists so the kernel gets the option rather than discovering the divergence later.
- The cost is real: finer grain means more rows to keep honest, and a registry that lags the code
  is worse than no registry. That is the argument against, and it may win.
