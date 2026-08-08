---
id: M-76
title: Bound the inbound audio queue in time
pillar: Media
status: ready
priority: 1
design:
epic: demand
areas: [sipx-media, sipx-audio]
predicate:
announcement:
note: M-45 measured the jitter buffer and cleared it · the inbound channel holds 256 frames, which is 5.12 seconds of audio with no time bound, counter or shed policy
---

# Bound the inbound audio queue in time

## Goal

Put a stated time bound, a counter and a shed policy on the queue between the media session and the
application, which is where seconds of added delay actually accumulate.

## Acceptance

- [ ] The inbound audio queue's bound is expressed in time rather than in frames, so the same
      configuration means the same delay at every packet duration.
- [ ] A failing-first test proves an application reading slightly slower than real time settles at a
      bounded delay rather than at the far end of the queue and staying there.
- [ ] Overflow is a stated policy — shed oldest, shed newest, or backpressure — chosen deliberately,
      documented in `docs/specs/media-runtime.md` §4, and counted in `MediaDiscardCounts` like every
      other media discard.
- [ ] The delivery contract change is stated in `CHANGELOG.md` with migration guidance: this alters
      what every application sees when it reads slowly.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `M-45`'s measurement, which is the point of that story. `M-45`
  characterised the jitter buffer across ten 1,500-packet traces and **cleared it**: bounded, no
  ratchet, strictly ascending across the wrap, late and duplicate refused, worst measured hold 515 ms
  under a 300 ms spike every third packet. The delay is downstream. `crates/sipx-media/src/session.rs`
  holds inbound audio in an `mpsc::channel::<Vec<i16>>(256)`, one frame per packet, delivered with
  `send().await` — at 20 ms that is **5.12 seconds** of audio that can queue before backpressure
  reaches the socket, with no bound in time, no counter and no shed policy. That fits the two field
  reports of "seconds of added delay" far better than the buffer does.

## Notes

- `M-45` deliberately did not touch it: changing that queue's depth or policy changes the delivery
  contract for every application, and deserves its own story and its own measurement.
- `M-45` also left the buffer's depth counted in **packets** rather than time — the same wall-clock
  jitter costs a 40 ms mean hold at 20 ms ptime and 119 ms at 60 ms. That is a sizing inconsistency
  rather than the unbounded growth this story is about, but the two decisions should be made with
  the same unit in mind.
