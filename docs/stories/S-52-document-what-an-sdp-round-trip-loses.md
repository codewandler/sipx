---
id: S-52
title: Document what an SDP round trip loses
pillar: Signalling
status: done
priority:
design:
epic: sip-core
areas: [sipx-sdp]
predicate:
announcement:
note: parse plus to_string_sdp is lossy in ways nothing states · harmless for a description sipx authors, fatal for one it relays
---

# Document what an SDP round trip loses

## Goal

State, at the call site, what `parse` followed by `to_string_sdp` does not preserve — so the next
caller who relays a description instead of authoring one finds out from the documentation rather
than from a peer.

## Acceptance

- [x] `to_string_sdp`'s documentation names every field the round trip does not preserve: `v=`,
      `o=` and `c=` nettype and addrtype, multicast `/ttl`, `m=` port `/count`, line order and
      whitespace.
- [x] It states the consequence plainly — safe for a description this stack authored, unsafe for one
      received and forwarded — and points at `sipx-sdp`'s relay path as the supported way to forward.
- [x] A test pins the loss set, so a future parser change that silently preserves or drops something
      else fails rather than quietly widening the contract.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `C-7`'s adjacent findings. That story had to rewrite description text in a
  new `crates/sipx-sdp/src/relay.rs` precisely because the round trip is lossy, and noted that a
  note on `to_string_sdp` would have saved the discovery.

- 2026-08-08: implemented. `to_string_sdp` now carries a "This is not a round trip" section naming
  each dropped field — the reissued `v=`, `o=`/`c=` nettype and addrtype, multicast `/ttl` and
  address count, `m=` port `/count`, line order and whitespace — and states the consequence:
  safe for a description this stack authored, unsafe for one it received, with `crate::relay` named
  as the supported way to forward. `crates/sipx-sdp/tests/round_trip_loss.rs` pins three of those
  losses against a fixture that carries all of them, so a parser change that starts preserving one
  fails here rather than surprising the next caller who forwards someone else's description.

## Notes

- This is documentation and a pinning test, not a parser change. Making the round trip lossless is a
  different and much larger story, and may not be desirable — the current shape is fine for the
  authoring case it was built for.
