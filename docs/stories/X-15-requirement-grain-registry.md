---
id: X-15
title: Consider requirement-grain rows in the RFC registry
pillar: Build
status: backlog
priority:
design: docs/designs/rfc-registry-grain.md
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
- **Decided: per-RFC grain stays.** Requirement-grain rows are declined for the kernel. The
  decision, its evidence and its reopen triggers are in
  [`docs/designs/rfc-registry-grain.md`](../designs/rfc-registry-grain.md).
- What settled it, in order of weight:
  - **Not verifiable.** The existing header/method checks bind to the parser's name table — you
    cannot satisfy them without the code. Nothing comparable exists for a section: 1053 `§`
    citations live in Rust comments, against 2 function names. The strongest checkable variant
    was measured rather than assumed — 30 of 32 (entry, section) pairs already trace into their
    cited files, so it nearly works — but it binds to a comment, and the 2 that fail it are
    5626 §5 and 9001 §9.2, exactly the sections cited as *not* implemented. It would flag the
    honest negative claims and pass anything with a section number typed in a comment.
  - **Nothing overclaims today.** All 15 `partial` entries name their gap; 10 of 33
    `implemented` entries name a limitation, and the gap is carried by *another row* — 9 of the
    12 RFCs cross-referenced from notes have their own entry (RFC 3311, named as the gap in both
    3262 and 4028, is a `syntax` row and the top unstarted roadmap item). The other 3 are out of
    the registry's stated bounds, not concealed.
  - **Three of the four coverage kinds already exist**: `syntax` vs `implemented` *is* the
    parse/behave distinction, and `roles` is role coverage. Only interop is absent, and that is
    a property of a test run, not of a row.
  - **Cost is real**: the registry is touched by 12 of 85 commits, and evidence is 85 bare file
    paths with no `file::test` form even at today's grain.
  - **The offer is not load-bearing**: the downstream ledger already records requirement grain as
    a local extension with kernel rows inherited by reference, so adopting it here unblocks
    nothing there.
- Code the decision implies — a decision left as prose would have rotted the first time someone
  added a row:
  - `scripts/rfc-report.py` gains `schema_problems`: the key set is closed, so an unknown key
    (`[[rfc.requirement]]`, or `role` typed for `roles`) fails the gate with a message naming the
    design doc. Previously such a key was parsed and silently dropped — the claim sat in the
    source, never reached the generated table, and nothing said so. `main` now reports shape
    before rendering, since `render` would otherwise crash on a malformed entry.
  - `docs/rfc/README.md` documents the schema as a contract a downstream can pin: row identity,
    the closed key set, status-vocabulary stability, and pin-a-tag. This is Acceptance item 4 —
    inheritance needed a stable *form*, not finer content.
  - `scripts/test-rfc-report.py` — first test for the script. 8 tests.
- `docs/compliance.md` is byte-identical: the change adds checks, not output.
- Loose end left deliberately: `test-rfc-report.py` is not wired into `.github/workflows/ci.yml`,
  since the gate's composition was out of scope for this story.

## Notes
- Offered by [sipx-clstr](https://github.com/codewandler/sipx-clstr), which is building an
  independent registry instance on this schema for a different role set (proxy, registrar) and has
  already decided to extend locally rather than wait
  ([ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md)). This story
  exists so the kernel gets the option rather than discovering the divergence later.
- The cost is real: finer grain means more rows to keep honest, and a registry that lags the code
  is worse than no registry. That is the argument against, and it may win.
