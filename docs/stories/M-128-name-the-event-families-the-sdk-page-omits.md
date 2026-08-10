---
id: M-128
title: Name the event families the SDK page omits
pillar: Application
status: ready
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

- [ ] The page either names every event family the contract carries, or says in its own words that
      the list is illustrative and points at the normative table.
- [ ] Whichever is chosen is *checked*: a generated region, or a test that fails when a family
      exists in the contract and appears in neither the page nor an explicit exemption. Prose that
      can silently fall behind is what this story is about.
- [ ] The check names what is missing, not merely that something is.
- [ ] The gate is green.

## Progress

- Filed 2026-08-10 by the `M-108` implementor, who found the omission and judged a docs sweep to be
  outside a behavioural story's fence.
