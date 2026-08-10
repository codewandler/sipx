---
id: M-121
title: Follow private types in the raw-audio carrier reader
pillar: Media
status: in-progress
priority: 3
design:
epic:
areas: [tooling, audio, security, media]
predicate:
announcement:
note: after M-68 · the checker reads one level deep, and M-68 found a carrier two levels down
---

# Follow private types in the raw-audio carrier reader

## Goal

Make `scripts/check-audio-claims.py`'s raw-audio rule catch a public `Debug` that reaches call audio
through a private field, which is the shape `M-68` found by hand after the checker passed.

## Acceptance

- [x] The carrier reader resolves a public type's fields through private types in the same crate,
      to a stated depth, and reports a `Debug` that can reach a PCM buffer however many hops away.
- [x] `M-68`'s own finding is a regression fixture: a public `Debug` over a private struct holding a
      `Vec<i16>` is reported, and the same type with a hand-written `Debug` is not.
- [x] Whatever the widened reader newly reports across the workspace is either fixed or argued at
      its site; the check does not land reporting known-unfixed carriers.
- [x] The docstring states the new depth, why that depth, and what a carrier past it still costs a
      reviewer — the check narrows deliberately or not at all.
- [ ] The full gate is green.

## Progress

- Backlog. Filed by `M-68`, which found `sipx_media::dsp::DspGraph`'s derived `Debug` rendering
  every sample of the frame in flight, the staging buffer and the scratch region — and up to eight
  more frames from a supervised stage's buffer pool. The checker passed the tree that contained it,
  because `DspGraph` holds no PCM field of its own: it holds a `SlotRef`, which holds an
  `Arc<Mutex<Slot>>`, which holds the buffers. `M-107`'s reader looks at a public type's own fields,
  which was the right first cut and is one hop short of this.

- 2026-08-10: implemented on `impl/M-121`. Four of five rows; the gate row is the coordinator's.

  **Failing-first.** `TheCarrierChase` in `scripts/test-audio-claims.py`, eighteen tests, run at
  `git merge-base main HEAD` = `c43ebde` with the reader unchanged: `Ran 18 tests … FAILED
  (failures=7, errors=3)`. The two that state the Acceptance:

  ```
  FAIL: test_a_public_debug_over_a_private_carrier_is_reported
  `M-68`'s finding, as the Acceptance states it: one hop, and nothing else.
      self.assertEqual(1, len(problems))
  AssertionError: 1 != 0

  FAIL: test_the_four_hop_chain_m68_found_by_hand_is_reported
      self.assertEqual(1, len(problems))
  AssertionError: 1 != 0
  ```

  The eight that passed at the base are the *does not report* half — over-reporting is the failure
  mode a widening introduces, and those only acquire teeth once the chase exists.

  **The depth is six hops, and it is the defect's number rather than the workspace's.** `M-68`
  found `DspGraph` rendering call audio at two distances: the frame in flight four hops down
  (`DspGraph → SlotRef → Slot → Live → Buffers`) and a supervised stage's pool of up to nine more
  frames at six (`Live → Stage → Running → Supervised`). Six covers the whole finding rather than
  half of it. Measured against the workspace as a check on that choice and not as the reason for
  it: the report saturates at four hops and the carrier population at six, and seven and eight
  select nothing further. What is past it is stated in `_CARRIER_HOPS` — a seventh hop is not
  reported and no red gate would say so, and the printed population is the only thing that moves
  when the workspace grows one.

  **A second defect had to be fixed first, and it is the more alarming of the two.**
  `check-audio-claims.py`'s `code()` cut every file at the first `#[cfg(test)]` *anywhere*, and a
  test-only item carries that attribute at an inner indentation. `crates/sipx-media/src/dsp/graph.rs`
  declares one on a constructor at line 964 of 2,552 — so `Stage`, `Buffers`, `Live`, `Slot` and
  `SlotRef`, which is the entirety of `M-68`'s chain, were invisible to *every* rule in the file,
  not just this one. Twenty-four such attributes across twelve files in five crates. None of the
  twenty-four is a test module; every real test module in this workspace sits at column zero, so
  anchoring the cut there cuts exactly what the docstring always claimed. Measured before and
  after over the whole workspace: exit 0 both ways, no new problem in any rule, and one debt count
  moves — byte-buffer carriers outside the relay-path scope, 50 → 51.

  **What the widened reader then found: the same defect a third and fourth time, in the two most
  loggable public types in `sipx-media`.**

  1. `MediaSession` → `Arc<InboundQueue>` → `Mutex<State>` → `frames: VecDeque<Vec<i16>>`. Every
     type on the way derived `Debug`, so `{session:?}` rendered the whole inbound queue — up to
     `Config::DEFAULT_INBOUND_QUEUE`, a fifth of a second of the far end's conversation, at a
     length that is the audio's. `PcmCapture` borrows the session and rendered it too.
  2. `PcmProcessor` → `Arc<Queue>` → `Mutex<QueueState>` → `frames: VecDeque<Offered>` →
     `samples: Vec<i16>`. Same shape, one attachment's whole bound of undelivered frames.
     `ActivityWiring` holds a `PcmProcessor` and rendered it too.

  Fixed at the two leaves, which is where `M-68` wrote its own: hand-written `Debug` on
  `inbound::State` and on `processing::Offered`, both reporting counts. That cuts all four chains,
  and the run is clean at 28 carriers. Each redaction has a test beside it asserting what it
  *prints*, which no checker can do —
  `a_session_renders_its_queue_depth_and_not_the_audio_waiting_in_it` and
  `an_attachment_renders_its_depth_and_not_the_frames_it_is_holding`. Both hold the record's length
  against an equivalent object holding no audio rather than against a constant: a `MediaSession`
  renders 5.7 kB of shape, and what this rule is about is whether any of that length is the call's.

  **Two narrowings, and which way each fails.** `OPAQUE_CONTAINERS` is the one kind of field the
  chase does not follow — a channel's `Debug` is the channel, so `MediaSession`'s
  `mpsc::Sender<Frame>` carries no chain. It is a *deny* list precisely because an allow list would
  fail silently at exit 0 on a wrapper nobody listed, where this fails by reporting a type whose
  `Debug` was never going to print the buffer. `resolve` prefers a declaration in the same file and
  otherwise follows every declaration of the name: `sipx-audio` has three private `Band`s and only
  G.722's holds a delay line, and a flat resolution reported the peaking filter for a buffer in a
  codec it has never heard of.

  **CHANGELOG sentence owed** (fenced; the coordinator writes it):

  > The raw-audio guard now follows a public type's fields through the crate's private types, six
  > hops deep, so a derived `Debug` that reaches call audio indirectly is caught rather than
  > reviewed by hand; `MediaSession` and `PcmProcessor` no longer render the audio waiting in their
  > queues.
