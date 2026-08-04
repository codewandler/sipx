---
id: X-71
title: Scope the provenance relaxation for comparison subjects
pillar: Build
status: done
priority: 14
design: docs/designs/stack-comparison.md
epic: stack-comparison
areas: [scripts, docs]
predicate:
announcement:
note: reverses non-negotiable 1 in one directory and supersedes X-47's site-neutrality clause · must land alone
---

# Scope the provenance relaxation for comparison subjects

## Goal

Record, as an enforced boundary rather than a sentence, that comparison subjects may be named in
`docs/comparison/` and the documents generated from it — and nowhere else.

## Acceptance

- [ ] `AGENTS.md` non-negotiable 1 states the exception and its exact scope: comparison subjects in
      `docs/comparison/`, the internal `docs/comparison.md`, and the public page generated from
      them. Design rationale still cites RFCs and our own specs; `docs/vision.md` principle 5 is
      **not** amended.
- [ ] `scripts/check-provenance.sh` implements the exception as a **path allowlist on the file
      scan only**. A denylisted term in any other tracked file still fails, proven by a test that
      writes one outside the scope and asserts exit 1.
- [ ] **The history scan stays absolute.** `--history` denies a denylisted term in any commit
      message, with no exception, proven by a test. This is what keeps the irreversible failure
      mode — "history must be rewritten before this repository is published" — unreachable.
- [ ] The allowlist is a constant in the script with a comment naming this story and the reason,
      not a glob assembled at call time. Widening it must require editing the check.
- [ ] `X-47` gains a `note:` recording that its "no prior-art project names in the README or public
      site" criterion is superseded by this story. **`X-47`'s status, Acceptance and history are
      not edited** — a closed story is a record.
- [ ] The superseded rationale is written up in `docs/archive/` per existing convention, stating
      what the vendor-neutral-site decision was for and why it changed.
- [ ] `./scripts/check-provenance.sh` and `./scripts/check-provenance.sh --history` both clean on
      the tree; `./scripts/gate.py` green.

## Progress

Implemented 2026-08-04. Everything in Acceptance is done except the final full-gate run.

- **`COMPARISON_SCOPE`** added to `scripts/check-provenance.sh` as a named array of three
  pathspecs, with the reason and the widening rule in a comment above it. Passed to `git grep` in
  the file scan only. The no-git fallback branch gets a coarser `--exclude-dir` approximation, and
  the comment says so rather than implying parity.
- **The history scan was left untouched.** `COMPARISON_SCOPE` appears at three places in the file
  (the header comment, its definition, the file scan) and nowhere after `scan_history -eq 1` —
  asserted structurally by `TheScriptItself.test_the_history_scan_is_not_subject_to_the_scope`.
- **Boundary verified both directions before the suite existed**, using tracked probe files in the
  real repository, since `git grep` ignores untracked ones: a subject name in
  `docs/specs/_probe-out-of-scope.md` failed with exit 1 and named the file; the same name in
  `docs/comparison/observations/_probe.json` passed with exit 0. Both probes removed and unstaged.
- **`scripts/test-provenance.py`** added — 13 tests, all passing. It builds throwaway git
  repositories and supplies its own denylist, so it needs no access to the private one. The term it
  uses is deliberately fictional: a real one written into a tracked test file would be caught by the
  check under test.
  - the three in-scope paths each pass, driven from one table;
  - a spec, a source file, and a design doc whose *filename* merely contains "comparison" each fail;
  - an in-scope file does not excuse an out-of-scope one in the same tree;
  - a commit message naming a subject fails `--history` **even when every file is in scope**;
  - missing denylist: exit 2 under `CI`, exit 0 with a skip locally;
  - `COMPARISON_SCOPE` is a named constant listing exactly the three artifacts, so widening it
    fails a test and needs a story.
- **`AGENTS.md` non-negotiable 1** amended: the exception, its path scope, and the two things it
  does not cover — design rationale (`docs/vision.md` principle 5 unchanged) and commit messages.
- **`X-47`** carries a superseded-in-part note; its status, Acceptance and body are unedited.
- **`docs/archive/2026-08-04-vendor-neutral-public-site.md`** records what the decision was, the
  three reasons behind it, which two survive, and the conflation that unpicked the third. First
  file in `docs/archive/`, which held only `.gitkeep`.
- **Registered**: `Step("provenance tests", "gate", …)` in `scripts/gate.py` and the mirrored
  `run:` in `ci.yml`'s gate job. `./scripts/gate.py --check` reports **33 steps over 19 CI jobs,
  none unaccounted for**; `test-gate.py` 92 tests pass.
- **Also green**: `check-provenance.sh` and `--history` on the real tree, `check-docs-links.py`
  (508 links), `maturity.py --check`, `rfc-report.py --check`.

- **Full gate: `gate: 33 steps, all green`.** Deferred at first because the tree carried uncommitted
  Rust changes from other work and a red cargo step could not have been attributed here. That
  cleared when `2794bee` and `0ae603d` landed mid-session: zero modified `.rs` files remained, the
  tree held only this story's work, and the run became attributable. `docs site` reports 14
  generated regions in sync and the anchor guard armed.

All Acceptance items are satisfied.

## Notes
- **This story lands alone.** Mixing a policy reversal into an implementation diff is how a policy
  change becomes invisible; the diff should be readable as "we decided this".
- The peers already named in `tests/interop/` were never on the denylist — the site was kept
  vendor-neutral by product decision, not legal constraint. Both facts belong in the archive note so
  a future reader does not conflate them.
- The denylist itself stays out of the repository, supplied by `SIPX_DENYLIST` /
  `SIPX_DENYLIST_FILE`. Nothing here changes that.
