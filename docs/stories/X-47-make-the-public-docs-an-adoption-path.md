---
id: X-47
title: Make the public docs an adoption path instead of a status ledger
pillar: Build
status: done
priority: 1
design: docs/vision.md
epic:
areas: [docs]
predicate:
note: shorten the front doors, lead with shipped CLI and Rust workflows, make security and operational limits canonical, and demote experimental surfaces
---

# Make the public docs an adoption path instead of a status ledger

## Goal
Make the README and public website answer what sipx is, whether it fits, and how to complete a first
working task before they expose implementation history or experimental direction.

## Acceptance
- [x] The README is a concise front door with explicit pre-1.0 status, CLI limitations, separate CLI
      and Rust paths, a measured capability summary, and links to canonical detail rather than repeated
      internal rationale.
- [x] The website leads with shipped workflows. Its navigation separates start, CLI, Rust library,
      reference and experimental material; the SDK is visibly experimental rather than a top-level
      promise.
- [x] Getting started names the MSRV, distinguishes the tagged release from `main`, gives a bounded
      two-terminal call, and says that the CLI consumes WAV files rather than a sound device.
- [x] Security and troubleshooting each have one canonical public page. The security page distinguishes
      the CLI from the Rust library; troubleshooting covers advertised addresses, NAT, WAV, auth,
      diagnostics and sensitive captures.
- [x] Product-specific migration material is replaced by one vendor-neutral guide grounded in SIP roles
      and RFCs, with no prior-art project names left in the README or public site.
- [x] Public facts that already have machine-readable sources — release, MSRV, crate map and RFC count —
      are checked or generated instead of copied without a guard. Public pages contain no internal story
      IDs or links into story/design files.
- [x] The answer-a-call guide demonstrates the complete in-dialog lifecycle rather than describing a BYE
      handling obligation its example does not satisfy.
- [x] `./scripts/build-docs.sh` and `./scripts/gate.py` pass.

## Progress
- Story filed after a read-only audit of the README, website source, CLI help, package metadata and docs
  build. The current site builds and its four inlined examples are synchronized; the problem is reader
  flow and semantic drift rather than broken rendering.
- Rewrote the README and website around shipped CLI and Rust entry paths; added security,
  troubleshooting and vendor-neutral integration guides; demoted the SDK; and published the generated
  compliance table on-site.
- Extended `sync-website.py` to generate/check release, MSRV, crate and RFC facts and to reject internal
  tracking material in public pages. Six focused unit tests run from the docs build.
- Updated the compiled answer example to drive the full in-dialog lifecycle through `serve`. The bounded
  two-process README smoke test reported `answered` on both sides and cleaned up its process group.
- `./scripts/build-docs.sh` and the complete 22-step `./scripts/gate.py` passed on 2026-07-30.

## Notes
- `sipx-host` exists, but the public SDK page and package description still say there is no host process.
  The canonical SDK status must distinguish the host from the still-missing app callback bindings.
- The track plugin command is unavailable in this environment, so the board row is regenerated in the
  same format as the surrounding generated region.
