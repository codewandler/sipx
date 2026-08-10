---
id: C-10
title: Answer a delayed offer in the 2xx and its answer in the ACK
pillar: Signalling
status: in-progress
priority: 7
design: docs/designs/edge.md
epic: edge
areas: [sipx-call]
predicate:
announcement:
note: RFC 3264 §5's other delayed-offer carrier · the only one available to a peer with no 100rel, and the reason C-8 still refuses that INVITE
---

# Answer a delayed offer in the 2xx and its answer in the ACK

## Goal

Let a UAS answer an offerless INVITE by putting its own offer in the `2xx` and adopting the answer
carried by the ACK, so that RFC 3264 §5's delayed offer works for a peer that does not support
`100rel` — and so the off-media coupling can relay that shape too.

## Acceptance

- [x] `answer` on an offerless INVITE originates the offer in the `2xx` instead of refusing `400`,
      and the answer arriving in the ACK settles the session, with a failing-first test that plays
      audio to the port that answer named.
- [x] A malformed or unnegotiable answer in the ACK ends the dialog rather than leaving a
      confirmed call with no agreed session — the ACK has no response with which to refuse one.
- [x] `OffMediaCoupling` relays that shape: an offerless source INVITE with no `100rel` is relayed
      as an offerless target INVITE, the target's `2xx` offer is mapped onto the source `2xx`, the
      target ACK is held until the source ACK supplies the answer, and that answer is mapped onto
      it. The `C-7` invariants hold unchanged: no `MediaSession`, no RTP bound, no sipx address,
      and an unmappable description refused before the peer leg is told.
- [x] The held target ACK has a bound: a source that never ACKs must not leave the target
      retransmitting its `2xx` for the full 32 seconds and then tearing down a dialog this side
      already reported.
- [x] `docs/specs/call-coupling.md` §6.3 is replaced by the specification of the carrier, and
      `docs/rfc/registry.toml`'s RFC 7092 note stops naming it as the §3.1.3 gap.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-10: delivered endpoint vectors IOF-5 through IOF-7 and coupling vectors T9 through T11.
  A dispatcher-owned answer now retains its routed ACK inbox, starts media only after a valid ACK
  answer, and sends BYE on malformed or unnegotiable answers. The off-media role maps the target
  final offer into the source 2xx, holds the target ACK for the mapped source answer, and bounds
  that hold with deterministic ACK-and-BYE cleanup. Focused integration suites passed (34 call
  vectors and 12 off-media coupling vectors), as did strict clippy for the library and both
  changed test targets.
- 2026-08-10: the complete local acceptance gate passed all 51 steps on the rc.23 candidate.

- 2026-08-10: failing-first endpoint vector `IOF-5` reached `answer` with an empty initial INVITE.
  The answering future returned `Error::Sdp("missing the o= line")`, the peer received **400**,
  and the assertion requiring the delayed-offer **200** failed. This pins the existing reversal:
  the empty body is parsed as a broken offer instead of causing this UAS to originate one.

- 2026-08-10: failing-first off-media vector `T9` returned immediately with
  `Error::Sdp("an offerless INVITE with no 100rel leaves no carrier for the target's own offer")`;
  no target invitation appeared in the ten-second failure bound. This pins §6.3's old refusal
  before the final-response carrier exists.

- 2026-08-08: filed from `C-8`, which relays every early carrier that RFC 3262 makes available and
  found this one unreachable. `C-8` mirrors `100rel` onto the target INVITE rather than asserting
  it, because a description arriving in a reliable provisional can only be carried onward if the
  source leg accepts one too. A source that offers no `100rel` therefore leaves RFC 3264's other
  carrier — the offer in the `2xx`, the answer in the ACK — as the only one, and `answer_tagged`
  refuses an offerless INVITE `400` before reaching it. With no sipx endpoint on either side of
  that shape, `C-8` could not have written a live causal proof for a relay of it, so it refused the
  INVITE `488` rather than shipping the carrier unproven. This story builds the endpoint half
  first, which is what makes the relay half provable.

## Notes

- The UAS half is `answer_tagged` in `crates/sipx-call/src/call/mod.rs`, which parses the INVITE
  body and refuses `400` when there is none. `Call::accept_delayed_offer_answer` already exists for
  the *re-INVITE* case in `crates/sipx-call/src/call/reinvite.rs` and is the shape to follow.
- The relay half is `OffMediaCoupling::dial` and `confirm_target` in
  `crates/sipx-call/src/coupling/transparent.rs`. `confirm_target` ACKs the target's `2xx` as soon
  as it arrives; this carrier needs that ACK held, and every failure path after it still has to
  send one.
- `docs/specs/call-coupling.md` §6.3 records exactly what is refused today and why.

- 2026-08-08: renumbered from `C-9` on filing. `C-9` was allocated in the same wave by `C-6`, for
  carrying a bridge across a renegotiation. Only the id moved; this remains the endpoint-half story
  that has to land before the `2xx`/ACK carrier can be relayed with a live proof behind it.
