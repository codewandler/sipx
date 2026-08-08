---
id: X-112
title: Take the integrated gate green on main and reconcile derived artifacts
pillar: Build
status: done
priority:
design:
epic: release
areas: [ci, scripts, website, docs]
predicate:
announcement:
note: the single deferred acceptance row 29 stories share · five steps are already proven red at the merge base
---

# Take the integrated gate green on main and reconcile derived artifacts

## Goal

`main` carries the complete post-`rc.2` wave, and twenty-nine of its stories hold exactly one open
acceptance row: the integrated repository gate plus derived regeneration, deferred by working-session
instruction to one shared boundary. Produce that boundary — one green `./scripts/gate.py` on `main`
with every generated artifact regenerated from its source — so those rows can close against evidence
rather than assertion.

## Acceptance

- [x] The five steps `M-44` recorded red at merge base `df89424` are each resolved or reassigned:
      `test` (nine `sipx-cli` progress/env integration tests), `feature matrix` (the packaged-Opus
      usage literal), `comparison`, `comparison tests` and the `docs site` sync tests. A step that
      stays red leaves a filed story behind it, not a comment.
- [x] The stale comparison observation is refreshed so `scripts/comparison-report.py --check` passes
      on generated expiry and confidence, with no recorded run hash edited forward. Runs that the
      current contract invalidates are re-derived or removed, matching `X-102`'s precedent.
- [x] Every derived artifact regenerates clean and byte-exact against its checker: `maturity.py`,
      `rfc-report.py`, `check-audio-claims.py`, `check-cli-reference.py`, `sync-website.py --check`,
      `check-docs-links.py`, `check-published-onboarding.py` and `check-provenance.sh`.
- [x] One complete `./scripts/gate.py` run on `main` is green end to end, and its exact commit is
      recorded in Progress below. Free disk is confirmed at 12 GiB or more before the run; an exit-2
      disk abort is re-run after reclaiming space and is never recorded as a pass.
- [x] No story `status`, CHANGELOG entry or release note is edited by this story. The ledger is
      `X-113`'s and the tag is `A-38`'s; this story produces only the passing evidence they cite.

## Progress

- 2026-08-08: selected as the first ticket of the release-boundary wave. The five red steps are not a
  new discovery — `M-44` reproduced each of them unchanged at the merge base while implementing G.722,
  and recorded that none involve its own diff.

- 2026-08-08: **the boundary is produced.** `./scripts/gate.py` is green end to end at commit
  `3b22cec` — **37 steps, all green** — with 129 GiB free, so no disk abort was involved. The five
  steps proven red at merge base `df89424` each resolved to a real cause rather than a re-run:
  `test` was one defect, not nine — the CLI emitted ANSI escapes to a non-TTY stderr, splitting
  `event="call.ended"` so every automation assertion failed; `feature matrix` was a packaged-Opus
  check pinning the handwritten scanner's usage line after `X-110` replaced the parser; `comparison`
  and `comparison tests` were `P-22`'s `stop_signal` schema change invalidating both retained load
  runs, plus eight capability citations pointing at the `call.rs` that `X-67` split; `docs site`
  cascaded from comparison and then surfaced rustdoc reading `wav:<path>` as an unclosed HTML tag.
  Both load directions were re-measured under the current contract, 30 of 30 repetitions each
  through the 1,024 calls/s ceiling, and `dataset.json` was repointed — it still indexed the removed
  runs, which is why the new ones had not been validated at all.

## Notes

- The gate is `./scripts/gate.py` and owns the full step list; do not transcribe its steps into a
  command list that can drift from it.
- The comparison dataset expires by design, so a stale observation is the checker working, not a
  defect. The `compare-stacks` skill derives and refreshes that dataset under the confidence ladder.
- Sequence: this story runs after the in-flight implementation remainders (`M-44`, `T-39`, `M-70`,
  `M-57`, `A-25`) land, because each of them changes code the gate covers.
