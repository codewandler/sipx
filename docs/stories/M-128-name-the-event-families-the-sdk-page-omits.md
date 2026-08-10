---
id: M-128
title: Name the event families the SDK page omits
pillar: Application
status: in-progress
priority: 5
design:
epic: app-sdk
areas: [website, docs, app-sdk]
predicate:
announcement:
note: after M-108 · the page lists event families in prose and has not tracked the last several rows
---

# Name the event families the SDK page omits

## Goal

Make `website/docs/sdk/contract.md`'s prose list of event families either complete or explicitly
partial, so a reader cannot conclude that what it omits does not exist.

## Context

`M-108` added `call.leg.ended` and found that the SDK contract page lists event families in prose —
"playback, gather, recording, and dial completion; transfer progress; bridge state; and hold" — and
does not mention leg endings. It left the page alone, correctly: the page is explicitly
non-normative, it already does not enumerate all 26 rows, and `sync-website.py --check` is clean
because nothing generated is out of date.

That is a fair reason not to fix it inside another story, and a poor reason to leave it indefinitely.
A prose list that is silently incomplete is worse than an obviously partial one: a reader takes the
absence of leg endings, DSP transitions or signalling metrics as a statement about the contract
rather than as a sentence nobody updated. Several stories this cycle — `M-103`, `M-108`, and the DSP
transitions from `M-67` — added rows the sentence does not know about.

The narrow fix is to update the sentence. The durable fix is to make the omission impossible to
reintroduce, because the next event family will land the same way this one did.

## Acceptance

- [x] The page either names every event family the contract carries, or says in its own words that
      the list is illustrative and points at the normative table.
- [x] Whichever is chosen is *checked*: a generated region, or a test that fails when a family
      exists in the contract and appears in neither the page nor an explicit exemption. Prose that
      can silently fall behind is what this story is about.
- [x] The check names what is missing, not merely that something is.
- [ ] The gate is green.

## Progress

- 2026-08-10: selected in the five-story rc.22 wave.

- Failing first: `python3 scripts/test-sync-website.py` ran 33 tests and raised three errors because
  the sync tool had no `app_event_types`, `app_event_family_problems`, or
  `generated:app-event-families` support.
- Chose the durable complete-list branch. `sync-website.py` now reads every exact `call.*` entry from
  the normative §5.3 table, including rows that contain two event types, renders them into the SDK
  page, and links that inventory back to the normative table. A separate exact checker reports the
  missing names even when the generated region is absent or manually damaged; its mutation test
  proves the diagnostic names `call.leg.ended`.
- Focused verification: `python3 scripts/test-sync-website.py` passes all 33 tests and
  `./scripts/sync-website.py --check` reports all 28 generated regions in sync. The full repository
  gate remains for the release coordinator.

- Filed 2026-08-10 by the `M-108` implementor, who found the omission and judged a docs sweep to be
  outside a behavioural story's fence.
