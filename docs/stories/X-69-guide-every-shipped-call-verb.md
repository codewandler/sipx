---
id: X-69
title: Guide every shipped call verb
pillar: Build
status: in-progress
priority: 17
design: docs/designs/docs-depth.md
epic: docs-depth
areas: [website, sipx-call]
predicate:
announcement:
note: hold, transfer, DTMF, playback, recording and coupling all ship and appear only as bullets · follow-up
---

# Guide every shipped call verb

## Goal

Make every call verb sipx ships findable from the guides, so shipped work stops being
indistinguishable from unbuilt work to a reader of the site.

## Acceptance

- [x] A guide exists for each shipped verb not currently covered: hold and resume, blind transfer,
      attended transfer, sending and collecting DTMF, playback, recording, and two-leg coupling.
      Place, answer and register already have theirs.
- [x] Each guide's sample is inlined by `sync-website.py` from a real file under `crates/*/examples/`,
      compiled by CI like the existing four. Where no example file exists, this story writes one.
- [x] The three example files currently not surfaced on the site are either surfaced or, with a
      recorded reason, deliberately left internal.
- [x] No sample is hand-written into Markdown. `sync-website.py --check` passes byte-exactly.
- [x] `does-this-fit.md` links each claimed capability to its guide, so the fit list stops being the
      only place a feature is mentioned.
- [ ] `build-docs.sh` passes with no new `WARNING_EXCEPTIONS` entry; `./scripts/gate.py` green.

## Progress
- Seven guides added under `website/docs/guides/` — hold-and-resume, blind-transfer,
  attended-transfer, send-and-collect-dtmf, play-audio, record-a-call, couple-two-calls — each
  inlining a new example under `crates/sipx-call/examples/` through a `generated:example` region,
  wired into the sidebar's Rust libraries section. `does-this-fit.md` now links every verb it
  claims to the guide that shows it, plus register, as-a-library, the CLI reference, and the
  coupling guide.
- The three examples the design found unsurfaced, decided one by one:
  - `sipx-app-protocol/examples/canned_program.rs` — **surfaced** from the application host
    overview (`website/docs/sdk/overview.md`) by name with its run command, the same mechanism
    `browser_audio_proof` uses. Not inlined: its module doc links a design doc relatively, which
    the public-content check rightly rejects, and at ~360 lines it is a tour rather than a sample.
  - `sipx-testkit/examples/dump_sequences.rs` — **deliberately internal**: it prints and
    regenerates the committed fuzz seed corpus, contributor tooling coupled to repo-internal test
    infrastructure, not a capability a site reader can adopt.
  - `sipx-testkit/examples/issue-certs.rs` — **deliberately internal**: it writes the fixture CA
    for the interop harness (`tests/interop/run.sh`); publishing a guide around a test-only
    certificate authority would invite using it as a TLS how-to, which it must never be.
- Example-count note the story asks for: `cargo build --workspace --examples` grows from 10 to 17
  compiled examples; all seven are thin `sipx-call` binaries and add roughly a second to a warm
  workspace examples build — noticeable in the count, not in the wall clock.
- `sync-website.py --check` is byte-exact for every region this story owns. The two failures it
  still reports predate this branch and touch no file in it: the comparison page refuses to render
  while `comparison-report.py --check` is red (the parked comparison-dataset staleness), and
  `website/docs/reference/compliance.md`'s generated region is stale against main's regenerated
  `docs/compliance.md`. Same cause fails `test-sync-website.py` (2 errors) and therefore the
  `docs site` gate step on main itself. The site build proper is clean: no warnings, no dead
  links or anchors, `WARNING_EXCEPTIONS` still empty; `check-docs-links.py` and
  `check-published-onboarding.py --check` green.

## Notes (implementation)
- The last acceptance box stays open only because the gate is red on the merge base for the
  pre-existing reasons above; nothing this story adds fails a gate step.

## Notes
- Follow-up rather than beta-1: it is real competitive ground and a substantial content and example
  effort, and it does not make anything already published untrue.
- Watch the cost the design flags — every example added is compiled on every gate run by
  `cargo build --workspace --examples`. If the count becomes noticeable, say so in Progress rather
  than quietly dropping examples from the site.
- Pairs with `X-68`, which explains the layering; this one explains the verbs.
