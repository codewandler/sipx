---
id: M-90
title: Decide, before the freeze, whether a counter nothing can reach stays published
pillar: Media
status: backlog
priority: 7
design:
epic: media
areas: [sipx-media]
predicate:
announcement:
note: M-85 closed the only route to srtp_protect_failures · a published field that can no longer rise is the thing §4 argues against
---

# Decide, before the freeze, whether a counter nothing can reach stays published

## Goal

Settle what happens to `MediaDiscardCounts::srtp_protect_failures` now that no caller can make it
rise, while the answer is still cheap. `1.0.0` freezes the snapshot's shape; after that, a field
published stuck at zero is permanent.

## Why

`M-81` added the field because `M-79` had made the branch reachable: an extension whose length word
overstated its bytes produced a header longer than the packet, `SrtpContext::protect` refused it,
and the packet was dropped with nothing counting it. `M-85` then refused that extension one boundary
earlier, at `Packet::encode`, for both the out-of-bounds and the in-bounds case — so nothing
`Packet::encode` produces has a header the transform cannot read, and the only reachable route to
the counter is closed.

`docs/specs/media-runtime.md` §4 argues twice against exactly this shape: the ICE output naming no
bound socket "carries that reason instead of a field permanently stuck at zero", and so does the
SRTCP protect-error branch. `srtp_protect_failures` is now in that position, with two differences
that are why this is a decision rather than a deletion. It is **published** — it shipped, and
removing it is a breaking change to a snapshot type. And `protect` still has cipher-level failure
paths (`aead_seal`, `keystream`) that no argument in the spec closes, so "unreachable" is a claim
about the packet layer rather than about the function.

`M-81`'s own lesson applies to its own field: *"unreachable because of what the caller can be" is a
claim with an expiry date*. This story is that claim being re-examined once, deliberately, rather
than discovered again by whoever next makes the branch reachable.

## Acceptance

- [ ] One of three, argued in `docs/specs/media-runtime.md` §4: the field stays with its site and a
      restated reason; the field stays and the site carries a `// discard:` reason instead; or the
      field is removed with a `CHANGELOG.md` migration note saying what to read instead.
- [ ] Whichever is chosen, `docs/specs/media-runtime.md` §4's paragraph on the branch says what an
      operator should conclude from the number they see, including zero.
- [ ] If it stays, a test pins that the branch is still wired — a unit-level `protect` refusal, or
      an argument for why no test can reach it that does not repeat the reason `M-79` invalidated.
- [ ] `./scripts/gate.py` green.

## Notes

- Sites: `crates/sipx-media/src/counters.rs` (the field),
  `crates/sipx-media/src/session.rs` (the protect-error branch),
  `docs/specs/media-runtime.md` §4 (the argument this has to join).
- Related: `M-80` is the other freeze-deadline decision on this crate's published shape.

## Progress

- 2026-08-08: filed from `M-85`'s implementation, which closed the route and left the field behind.
  `M-85` kept both the field and the increment deliberately — it is not this story's job to be done
  in passing — and recorded the reasoning at the site and in §4.
