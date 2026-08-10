---
id: X-146
title: Put the media DSP subsystem on the architecture page
pillar: Experience
status: in-progress
priority: 9
design:
epic: conformance
areas: [website, docs, architecture, dsp]
predicate:
announcement:
note: found in a docs sweep at rc.19 · architecture.md predates the whole custom-call-DSP epic
---

# Put the media DSP subsystem on the architecture page

## Goal

Make `website/docs/architecture.md` describe the media stack that exists, including the DSP
subsystem an application can now attach, drive and be refused by.

## Context

Found by sweeping the published docs against everything merged since they were last touched, at the
`1.0.0-rc.19` boundary.

`website/docs/architecture.md` was last changed on 2026-08-05. Searching it for `dsp`, `graph`,
`noise` or `processor` returns four hits and every one is the mermaid keyword `subgraph`. Since that
date the custom-call-DSP epic landed in full: a bounded processor contract, an ordered per-direction
graph attached to a live call, nine built-in processors, interchangeable noise reduction, a
supervised isolation profile running a real operating-system process, an application control surface
that can move a live stage's parameters and bypass one stage, and a producer carrying a call's own
activity hint from its analyser to its reducer.

None of that is on the page a reader opens to understand how sipx is put together. The layer diagram
shows I/O drivers and protocol logic; the subsystem an application most directly extends is not
drawn at all.

This is a documentation gap rather than a defect, and it is the kind that compounds: the page is
where someone decides which crate to use, and a subsystem that is not on it will keep being absent
from the next reader's mental model no matter how many stories add to it.

## Acceptance

- [x] The layer diagram and its prose include the media DSP subsystem and say which crate owns it.
- [x] The page states the boundary that matters to a reader choosing a crate: what an application
      may supply, what it may not, and that a containment claim belongs to the execution profile
      rather than to the graph.
- [x] The "Which crate should I use?" section answers the DSP question, since that is the most
      likely new reason to reach past `sipx-call`.
- [x] No claim on the page is made that the specs do not already carry — the page summarises and
      links; it does not become a second normative source.
- [ ] The gate is green.

## Progress

- Filed 2026-08-10 from a sweep of published docs against all behavioural changes since their last
  update. Sibling of `X-145`, which covers the privacy page, and of `M-128`, which covers the SDK
  contract page's prose list of event families.

- 2026-08-10: implemented on `impl/X-146`.

  **Failing first.** `ArchitectureCoverageTests` was added before either public page changed. Its
  first run failed in seven subtests: neither DSP owner appeared in the layer diagram or either
  crate-selection table, and the architecture page linked none of the four specifications it now
  summarises.

  **The layer and the boundary.** The diagram now places `sipx-audio::dsp` with protocol/data logic
  and `sipx-media::dsp` with the live media driver. The prose follows the same split: the former
  owns the processor contract, conformance and built-ins; the latter owns call-local graphs,
  execution profiles and supervised workers. A Rust application may supply supervised or
  explicitly trusted work but may not grant itself the evidence-backed inline profile. The process
  contract remains narrower and can name registered identifiers, direction, shape and finite
  values, never code or execution policy. Containment is stated as a property of the profiles in a
  graph, including the uncontained cooperative profile, not as a property of graph membership.

  **A summary, not another contract.** The page links the normative processor, graph, effects and
  noise-reduction specifications directly, and says that it only locates their claims. Both crate
  tables answer the two distinct DSP questions with `sipx-audio::dsp` and `sipx-media::dsp`.
  Regression coverage requires those two owners in the diagram and both tables, and requires all
  four spec links on the architecture page.

  **Focused verification.** `scripts/build-docs.sh` passes: all 30 public-generator and content
  tests, 27 generated regions, the published consumer, 563 internal pages with 1,124 relative
  links, the site and anchor guard, and the API reference. `scripts/check-audio-claims.py --check`
  also accepts the expanded crate table. The full gate is the coordinator's wave gate, so the final
  acceptance row remains open.
