---
id: A-38
title: Publish and verify the second release candidate
pillar: Application
status: in-progress
priority: 3
design:
epic: release
areas: [release, docs, sipx-cli, rc3]
predicate:
announcement:
note: after X-113 · the post-rc.2 wave as one immutable prerelease · no stable-1.0 claim widens
---

# Publish and verify the second release candidate

## Goal

Publish the post-`rc.2` wave as `1.0.0-rc.3`: one immutable prerelease boundary carrying the fourteen
external-review fixes, bounded endpoint resolution, G.722 and the call-module split, under the same
protected evidence contract `A-37` used for `rc.2` — and without widening any claim about stable 1.0.

## Acceptance

- [ ] Public and internal documentation are swept together: workspace versions, adoption commands,
      generated comparison facts, roadmap status, changelog, What's New and reviewed release notes all
      agree on `1.0.0-rc.3`, while every historical record stays unchanged.
- [ ] `docs/releases/1.0.0-rc.3.md` describes the boundary the way the `rc.2` notes do — install
      pinning, what changed, fixed defects, measurement limits, experimental surfaces and intentional
      omissions — and states which external-review findings are now closed.
- [ ] The complete release gate passes on the frozen candidate. At most one local invocation is made;
      if infrastructure prevents a result, exact-SHA `main` CI and the protected tagged gate supply the
      passing evidence instead. `X-112`'s run is not repeated for its own sake.
- [ ] Exact-SHA `main` CI including Pages passes before one annotated `v1.0.0-rc.3` tag is pushed. The
      `v1.0.0-rc.1` and `v1.0.0-rc.2` tags remain immutable and are never moved.
- [ ] The protected workflow publishes or verifies every public crate, a registry-only consumer, the
      installed optional-feature CLI, all five native archives, SPDX documents, checksums, Pages and
      the non-draft GitHub prerelease from the reviewed notes.
- [ ] Registry checksums, workflow evidence, tag object, release commit, release URL and the portable
      asset set are recorded in this story before it closes.
- [ ] It remains a prerelease. No existing tag, package or asset is moved or overwritten; the v1
      predicate on independent application use is not claimed; and no broader announcement or
      external-review outreach happens as part of the cut.

## Progress

- 2026-08-08: selected as the exit of the release-boundary wave. `main` stands 61 commits ahead of
  `origin/main` at selection, and `Cargo.toml` still reads `1.0.0-rc.2`.

- 2026-08-08: **the candidate is cut locally.** `./scripts/gate.py` was green end to end at `3b22cec`
  (37 steps); the only changes after it were story frontmatter, the generated board, `maturity.md`
  and roadmap prose, and every checker those touch — maturity, sync-website, comparison, docs links
  and provenance — was re-verified green on the frozen tree. Annotated tag `v1.0.0-rc.3` now points
  at release commit `9e8bef8` with the workspace at `1.0.0-rc.3` and a clean tree.
- 2026-08-08: **publication is deliberately unrun.** `git push` and the protected registry workflow
  are the irreversible half and are held for explicit authorization, so the acceptance rows covering
  exact-SHA `main` CI, the pushed tag, registry packages, portable assets, Pages and the GitHub
  prerelease stay open. Nothing about `v1.0.0-rc.1` or `v1.0.0-rc.2` was moved or overwritten.

## Notes

- `A-37` owns `rc.2` and `A-10` owns the stable crate set; neither is superseded by this story.
- Push and publish are deliberate, separately authorized steps — never folded into preparation work.
- Model the notes and the evidence record on [`docs/releases/1.0.0-rc.2.md`](../releases/1.0.0-rc.2.md)
  and `A-37`'s Progress log.
