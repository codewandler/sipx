---
id: X-111
title: "Make published onboarding executable"
pillar: "Build"
status: in-progress
epic: published-adoption
areas: [docs, release]
design: docs/designs/published-adoption.md
note: "follow-up external review findings 6–7 · dependency snippets omit a used crate and the generated version renders truncated"
---

# Make published onboarding executable

## Goal

Turn the published library onboarding path into checked source: the displayed dependency list must
compile the displayed example from released packages, and generated release values must survive the
site renderer as complete prose.

## Acceptance

- [x] A failing-first clean consumer uses exactly the README/as-a-library dependency snippet and
      answer-a-call source, reproducing the missing direct package dependency.
- [x] The dependency block and example have one reusable source or a synchronization check. The
      registry-shaped consumer compiles without workspace path leakage or undeclared imports.
- [x] README, as-a-library and answer-a-call pages show a complete minimal dependency set, with
      exact-version policy consistent with the release channel and no hidden setup step.
- [x] A failing-first site assertion reproduces the getting-started sentence split and truncated
      workspace version from the inline generated marker.
- [x] Generated workspace-version content uses a markdown-safe form, and built HTML contains the
      exact complete sentence and version as visible text.
- [x] A repository check scans generated markers for unsupported inline placement so the same
      renderer defect cannot move to another page.
- [ ] Archived consumer, docs link/build, site rendering, package rehearsal and the complete
      repository gate are green.

## Review evidence

The follow-up review copied the published dependency and answer example into a clean consumer; the
example imported a package not declared by the snippet. The deployed getting-started HTML split the
version sentence into two paragraphs and displayed `.0.0-rc.2` instead of the complete version.

## Progress

- `docs/specs/published-onboarding.md` now defines the canonical clean consumer, exact dependency
  and source synchronization, generated-marker placement rule, built-visible-text assertion and
  ONB-1 through ONB-5 vectors. Board regeneration and the complete gate remain deferred to push.
- `tests/published-answer-consumer/` is the registry-shaped source of the generated dependency
  snippets and stays byte-identical to the compiled answer example. The focused consumer compile,
  five onboarding regressions, four synchronization regressions, documentation build, rendered
  HTML assertion, internal-link check and provenance check pass. The comparison-render portion of
  the existing synchronization suite remains blocked by comparative-load evidence that predates
  its current contract; package rehearsal and the complete gate remain deferred to push.
