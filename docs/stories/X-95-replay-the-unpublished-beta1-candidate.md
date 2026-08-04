---
id: X-95
title: Replay the unpublished beta.1 candidate under one-purpose authority
pillar: Build
status: in-progress
priority: 1
design: docs/specs/release-workflow.md
epic: conformance
areas: [release, ci, docs]
predicate:
announcement:
note: hard-bound historical completion after beta.2; beta.1 remains superseded and neither tag moves
---

# Replay the unpublished beta.1 candidate under one-purpose authority

## Goal

Publish the immutable `v1.0.0-beta.1` package set and GitHub prerelease after its original protected
run failed before any registry write, without turning partial recovery into first-publication
authority, moving either tag, presenting beta.1 as newer than beta.2, or hiding the candidate's known
documentation defect.

## Acceptance

- [ ] Failing-first helper and workflow mutations prove a third, mutually exclusive authority that
      is hard-bound to annotated tag object `b0bcadcc2a69a5824ec4a9549f7800c88c4f13fa`, release commit
      `3ab81709c7a235831638c62eba5fe73ce9eb7773` and failed run `30906820031`. Every other tag, run,
      workflow, ref, repository, event, controller identity or missing credential dispatches no
      upload; ordinary publication and partial recovery retain their existing authority exactly.
- [ ] The manual protected workflow proves the original step matrix, then reruns the complete gate
      on the separate immutable beta.1 checkout with the provenance input confined to that step and
      reruns the locked package rehearsal before exposing the Cargo credential. Controller files
      never enter a release archive.
- [ ] The pinned Cargo toolchain reproduces every visible beta.1 package before advancing a bounded
      dependency-ready frontier. Empty visibility is accepted only by this beta.1-specific replay;
      partial recovery still refuses to begin a publication. The remote annotated tag object is
      rechecked before every registry write and before the GitHub Release.
- [ ] All eleven exact beta.1 packages become crates.io-visible and byte-matching, then a clean exact
      registry consumer and crates.io-installed Opus CLI complete the bounded loopback proof.
- [ ] Historical Pages evidence is the unexpired `github-pages` artifact from exact-SHA CI run
      `30906258443`: its getting-started page names beta.1 and its `sipx-call` API index is present.
      The live site remains on beta.2 and is probed as the recommended release rather than rolled
      backward.
- [ ] The dependent least-privilege job creates or exactly verifies a non-draft GitHub prerelease
      targeting the beta.1 commit from reviewed replay notes. Those notes lead with “superseded”,
      name beta.2 as current, disclose `X-70`, and contain no broader announcement.
- [ ] Structural tests, focused helper tests and `./scripts/gate.py` pass. The run IDs, registry
      checksums and final GitHub URL are recorded here before the story is closed.

## Progress

- Protected run `30906820031` validated the exact annotated beta.1 tag and Cargo credential, then
  failed only because its gate step did not receive the configured `SIPX_DENYLIST`. Rehearsal,
  publication, consumer, Pages and GitHub Release steps were skipped; all eleven beta.1 versions
  remain absent from crates.io.
- Exact-SHA main CI run `30906258443` passed all jobs for the tag commit, including provenance and
  Pages deployment. Its unexpired artifact `8891214271` contains a getting-started page naming
  `1.0.0-beta.1` and the generated `sipx-call` API index. A current-controller locked dry-run against
  the separate beta.1 checkout packaged and verified all eleven public crates.
- This is not a claim that beta.1 was ready before beta.2. The immutable snapshot's generated report
  reads three of five and its dispatcher documentation contains the detached task and `expect`
  corrected by `X-70`. The replay record must preserve those facts and keep beta.2 recommended.
- The helper's failing-first suite initially reported eleven calls with an unknown replay-authority
  argument. It now has 56 green tests, including exact acceptance, every fixed-identity mutation,
  publish-only parsing and the unchanged partial-recovery guard. The workflow/checker suite has 35
  green structural and mutation tests spanning incident constants, checkout separation, original
  step evidence, current gate/rehearsal ordering, secret scope, bounds, historical Pages and the
  dependent non-latest prerelease.

## Notes

- A GitHub-only failed-candidate object would not satisfy this story: `released` still means exact
  registry distribution, consumer, CLI, Pages and GitHub evidence.
- A generic change allowing `--authorize-ci-recovery` to start from zero visible packages is out of
  scope and forbidden. The one-purpose replay authority becomes useless for any other tag or run.
