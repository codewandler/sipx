---
id: X-146
title: Put the media DSP subsystem on the architecture page
pillar: Experience
status: ready
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

- [ ] The layer diagram and its prose include the media DSP subsystem and say which crate owns it.
- [ ] The page states the boundary that matters to a reader choosing a crate: what an application
      may supply, what it may not, and that a containment claim belongs to the execution profile
      rather than to the graph.
- [ ] The "Which crate should I use?" section answers the DSP question, since that is the most
      likely new reason to reach past `sipx-call`.
- [ ] No claim on the page is made that the specs do not already carry — the page summarises and
      links; it does not become a second normative source.
- [ ] The gate is green.

## Progress

- Filed 2026-08-10 from a sweep of published docs against all behavioural changes since their last
  update. Sibling of `X-145`, which covers the privacy page, and of `M-128`, which covers the SDK
  contract page's prose list of event families.
