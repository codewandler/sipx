---
id: X-151
title: Refuse a stale changelog compare chain
pillar: Build
status: done
priority: 6
design: docs/specs/release-workflow.md
epic: release
areas: [release, docs]
predicate:
announcement:
note: found during release preparation · Unreleased still compared from rc.17 and five later release headings had no link definitions
---

# Refuse a stale changelog compare chain

## Goal

Make every release heading lead to the exact Git comparison it names, so the current changelog
cannot publish several immutable releases while its `Unreleased` link silently starts from an older
candidate.

## Acceptance

- [x] A failing-first test reproduces the current shape: the newest release is newer than the base
      of `[Unreleased]`, and at least one dated release heading has no reference definition.
- [x] The public synchronization check requires `[Unreleased]` to compare the newest dated release
      tag with `HEAD` and requires every dated bracketed release heading to have one reference
      definition.
- [x] Each release reference compares the preceding dated release tag with its own tag; the first
      recorded release may link directly to its release page.
- [x] The missing historical definitions and newest-release links are repaired without changing
      historical release prose or claiming an unpublished candidate is already published.
- [x] Focused synchronization tests, the live website check, links, provenance and the complete
      repository gate are green.

## Progress

- 2026-08-14: filed from the release audit. The changelog already contains rc.18, rc.19,
  rc.21, rc.22 and rc.23 sections, while its reference tail stops at rc.17. RC.20 was never
  published and has no release section, so the chain must follow recorded releases rather than
  assuming every numeric suffix exists.
- 2026-08-14: a four-vector focused suite passed, the live synchronization check accepted all 49
  dated release references, and the finalized content candidate passed all 55 repository gate steps
  in 16m37s.
