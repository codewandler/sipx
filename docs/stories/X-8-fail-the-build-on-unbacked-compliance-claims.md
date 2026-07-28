---
id: X-8
title: Fail the build on unbacked compliance claims
pillar: Build
status: done
priority: 2
design:
epic: conformance
areas: [docs]
note:
---

# Fail the build on unbacked compliance claims

## Goal
Make the compliance table impossible to leave stale, by checking it the way every other gate in
this repo is checked.

## Acceptance
- [x] `scripts/rfc-report.py --check` fails when the generated table is out of date.
- [x] It fails when an entry names a header or method the parser does not know.
- [x] It fails when an entry cites a file that does not exist.
- [x] It fails when an entry claims implementation and cites nothing at all.
- [x] CI runs it, and `AGENTS.md` lists it in the gate.

## Progress
- Done, wired into the `docs` CI job and into the gate in `AGENTS.md`.
- The last criterion is the one that matters most: an entry that says "implemented" and points
  at nothing is an assertion, and a table of assertions is what this whole story exists to
  avoid.
