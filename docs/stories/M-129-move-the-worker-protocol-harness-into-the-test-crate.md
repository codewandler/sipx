---
id: M-129
title: Move the worker-protocol harness into the test crate
pillar: Media
status: ready
priority: 6
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-media, sipx-testkit, dsp, fuzzing]
predicate:
announcement:
note: after M-122 · a fuzzing harness is public API of a published crate, against the convention this repo already wrote down
---

# Move the worker-protocol harness into the test crate

## Goal

Take the supervised worker-protocol fuzzing harness out of `sipx-media`'s public API and put its
vocabulary, oracle and corpus in `sipx-testkit`, leaving one opaque entry point behind — so the
fuzz target and the replay tests still drive exactly the same code, and a published crate stops
shipping a test harness.

## Context

`M-122` built the harness as a public module of `sipx-media`: `Side`, `Step`, `Program`, `Outcome`
and `Seed`, five types with public fields and variants, reachable from the crate root of a crate this
project publishes.

**Its reason was sound and is not what this story disputes.** The oracle's fourth rule — a refusal is
terminal in both directions — is only worth asserting against the real callers, `worker::serve` and
`supervised::pump`, and both are crate-private. Reaching a *model* of them from an external crate
would have meant publishing `read_message`, `Incoming`, `Fault` and three writers: more public
surface for a weaker claim. Given those two options it chose correctly.

The third option is that "the driver must live in `sipx-media`" and "the *vocabulary* must live in
`sipx-media`" were treated as one decision when they are two. The repository had already answered
this for its sibling harness, in a comment `M-122` added a dependency three lines beneath:

> The transaction-sequence harness — its event vocabulary, decoder, driver and invariant oracle —
> lives in the workspace's test crate rather than here, so the fuzz target and the regression tests
> in `crates/sipx-sip/tests/` drive exactly the same code. A corpus entry that crashes the fuzzer is
> replayed by a test byte for byte; two copies of the driver would make that a lie.

That property — one harness, two drivers — is the thing to preserve. It is not what makes the types
public.

There is a second, quieter cost. Five rationales had to be written into `check-audio-claims.py`'s
subject types (`/// Exhaustive by design:`, `/// Not the call:`, `/// Not a secret:`) purely because
those types are reachable from a published root. Every one of them disappears when the types stop
being public — and a rationale that exists only to answer a rule about surface an item should not
have had is a rationale that will be read, later, as a considered decision about audio.

`#[doc(hidden)]` was considered and rejected: it hides from rustdoc and changes nothing about semver
or about what the checker must be told. `#[cfg(fuzzing)]` was considered and is the fallback if the
split proves impossible — it removes the surface entirely, at the cost of code that ordinary builds
never type-check, and of moving the replay tests in-crate, which cuts against the one-harness
property above.

## Acceptance

- [ ] `Step`, `Program`, `Seed`, `Outcome` and the generator, oracle and seed corpus live in
      `sipx-testkit`, beside the harness that already set the precedent.
- [ ] `sipx-media` exposes at most one entry point for this, opaque enough that its signature does
      not re-export the vocabulary, and marked so a reader knows it is not application surface.
- [ ] The fuzz target and the replay test drive the same code, and a corpus entry that crashes the
      fuzzer is still replayed byte for byte — proved by a test, not by inspection.
- [ ] Every rationale added to the harness types for `check-audio-claims.py` is **removed**, not
      relocated: if one is still needed, that is a finding about the new arrangement and belongs in
      this story's Progress.
- [ ] `check-corpus-untouched.sh` follows the corpus to its new path and still fails on both
      modification and addition.
- [ ] The campaign is re-run at the CI budget after the move and finds nothing new; the throughput
      figure is recorded, since a harness that got slower crossing a crate boundary is worth knowing
      about.
- [ ] The gate is green.

## Progress

- Filed 2026-08-10, after `M-122` landed and its arrangement was reviewed at the release boundary.
  Recorded in `docs/releases/1.0.0-rc.19.md` under "Known and not fixed" rather than rushed into the
  candidate.
