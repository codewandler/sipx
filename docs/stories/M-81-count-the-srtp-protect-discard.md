---
id: M-81
title: Make the SRTP protect-error discard tell the truth
pillar: Media
status: ready
priority: 6
design:
epic: media
areas: [sipx-media, sipx-rtp]
predicate:
announcement:
note: a spec sentence says the branch is unreachable · M-79 made it reachable, and it increments nothing
---

# Make the SRTP protect-error discard tell the truth

## Goal

Bring the SRTP protect-error branch back into agreement with the rule `docs/specs/media-runtime.md`
states: every discard site either increments exactly one counter, or carries a reason no longer
capable of being false.

## Why

`docs/specs/media-runtime.md` says the SRTP and SRTCP protect-error branches need no counters
because "those branches receive bytes from `Packet::encode` and `Rtcp::encode_compound`, which
always make complete headers", and `crates/sipx-media/src/session.rs` repeats it at the site:
"structurally unreachable here".

That was true while every packet the send loop protected was built internally with no extension.
`M-79` made `Encoded.extension` public, so a caller can hand `send_encoded` an extension whose
embedded length word claims more 32-bit words than are present. `Packet::encode` forwards it
verbatim and sets the X bit, `rtp_header_len` then computes a header longer than the packet and
returns `None`, and `protect` returns `TooShort`. The branch is reached, one `warn!` is emitted, the
packet is dropped, and **no counter moves** — silent media loss on an encrypted leg, at the one site
the spec promised could not happen.

Bounded, and not a security defect: it is caller-induced rather than attacker-induced. The relay
path is safe, because `Packet::decode` bounds-checks a received extension. Nothing is weakened about
what SRTP authenticates or encrypts.

## Acceptance

- [ ] The site and `docs/specs/media-runtime.md` agree again. Exactly one of: a discard counter for
      the protect-error branch, validation of a caller-supplied extension at `send_encoded`'s
      boundary, or a restated reason that is true of the code as it now stands.
- [ ] A failing-first test hands `send_encoded` an extension whose length word overstates its bytes
      on an SRTP leg, and asserts whatever the chosen answer promises — a counter that moves, a
      typed refusal at the boundary, or the documented drop.
- [ ] Whatever is chosen holds for **SRTCP's** sibling branch too, or the difference is stated.
- [ ] `Encoded.extension`'s rustdoc stops promising that "whatever header extension the `Encoded`
      carries goes out on the same packet": an extension shorter than four bytes is already silently
      dropped by `Packet::encode`'s length filter, which the doc does not mention either.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from the independent review of `M-79`'s diff, with the exact reachability chain
  traced through `Packet::encode`, `rtp_header_len` and `SrtpContext::protect`.
