---
id: X-3
title: Enforce the provenance policy in CI and pre-commit
pillar: Core
status: done
priority:
design:
epic:
areas: [build]
note:
---

# Enforce the provenance policy in CI and pre-commit

## Goal
Guarantee mechanically that the published repository contains no reference to any
third-party prior-art project, so the guarantee does not rest on anyone remembering it.

## Acceptance
- [x] `scripts/check-provenance.sh` scans tracked files, and with `--history` the full
      commit log, for a case-insensitive denylist.
- [x] The denylist lives outside the repository — `SIPX_DENYLIST`, `SIPX_DENYLIST_FILE`, or
      `~/notes/sipx-research/denylist.txt` — so the terms we refuse to mention are not
      mentioned by the check itself.
- [x] An unconfigured denylist **fails** under `CI` (exit 2) rather than passing silently.
- [x] Wired into CI with `fetch-depth: 0` and into `.githooks/pre-commit`.
- [x] Verified against a term that is present (fails), one that is absent (passes), a
      comma-separated list, and the missing-denylist case.

## Progress
- Done. Found and fixed a parser bug on the way: `read` drops a trailing line with no
  newline, so a single-term `SIPX_DENYLIST` was silently discarded and the gate passed. Every
  read loop now uses the `|| [[ -n "$line" ]]` form.

## Notes
- Hook install is opt-in per clone: `git config core.hooksPath .githooks`.
- The CI job needs the `SIPX_DENYLIST` repository secret set before the first push, or the
  provenance job fails by design.
