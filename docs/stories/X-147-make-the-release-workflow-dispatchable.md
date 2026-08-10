---
id: X-147
title: Make the release workflow dispatchable
pillar: Build
status: in-progress
priority:
design:
epic: conformance
areas: [release, ci]
predicate:
announcement:
note: found while shipping rc.19 · GitHub rejects the release workflow before creating a job
---

# Make the release workflow dispatchable

## Goal

Make the protected release workflow valid at GitHub's dispatch boundary, and make the repository's
own checker refuse the expression-context mistake that prevented every release job from starting.

## Context

Found while shipping `1.0.0-rc.19`. Pushing its exact commit to `main` created a release-workflow run
with no jobs and the platform classified it as a workflow-file failure. An independent syntax check
resolved the defect to `GATE_TIMINGS`: the job-level environment tried to read `runner.temp`, but
runner context exists only once a step is executing.

That assignment was introduced with the release build cache and timing evidence. All 51 local gate
steps and all 51 release-workflow structural tests passed because the checker held the timing file's
location and consumers without holding the expression context that supplies the location. The
workflow therefore proved its release semantics while being impossible to dispatch.

`v1.0.0-rc.19` has not been pushed or published. It remains an unpublished boundary; the repair
must ship from a fresh version and annotated tag rather than moving that tag.

The first protected run from `v1.0.0-rc.20` created a job and proved that repair, then exposed the
next orchestration gap before package rehearsal or publication: the combined release gate lacked
the Windows cross target, both WebAssembly targets and the WebAssembly runtime that their separate
ordinary CI jobs provision. The tag remains immutable and produced no registry package or
prerelease. The provisioning repair therefore ships from `v1.0.0-rc.21`, not by moving rc.20.

## Acceptance

- [x] A failing-first release-workflow test puts a runner-only context in the release job's
      top-level environment and proves the structural checker refuses it.
- [x] The gate timings path is derived only where runner context exists, remains outside the
      checkout, and the gate, summary and preserved artifact all name the same file.
- [x] The ordinary and recovery workflow definitions pass an independent Actions syntax check.
- [x] A fresh RC boundary consistently names the new version in manifests, public docs, comparison
      evidence, changelog and reviewed release notes.
- [ ] The complete local gate passes, exact-SHA `main` CI including Pages passes, and the protected
      tag workflow publishes and verifies the registry packages, portable artifacts and GitHub
      prerelease.

## Progress

- 2026-08-10: filed from release run `31387911521`, which failed before job creation at the
  unpublished `1.0.0-rc.19` boundary.
- 2026-08-10: a failing-first mutation put `runner.temp` back in the job environment and reproduced
  the checker's blind spot. The repaired checker holds both expression scope and the identity of the
  timings file; 53 release-workflow tests and independent syntax checks of the ordinary and recovery
  workflows pass. Prepared `1.0.0-rc.20` as the fresh reviewed boundary.
- 2026-08-10: the 51-step local gate passed at `058db1d5d30d`. Exact-SHA main CI then exposed a
  separate hosted-runner provisioning gap: the native browser proof resolved its live-device
  dev-dependency but installed only its direct Opus and DTLS libraries. A failing-first harness test
  now holds the complete system-library set, and the proof job provisions it before compiling.
- 2026-08-10: exact-SHA main CI passed at `f821a43b1d0c` in run `31394483331`. Protected rc.20 run
  `31395349875` then passed immutable-tag validation and 48 of 51 cold-gate steps, but the Windows
  cross check and both WebAssembly checks reported absent runner tools. No rehearsal, registry write,
  portable build or prerelease step started. Failing-first mutations now remove the cross compiler,
  three Rust targets and runtime one at a time; the workflow checker refuses each omission, and the
  protected job installs them before the gate. Prepared rc.21 as the fresh reviewed boundary.
