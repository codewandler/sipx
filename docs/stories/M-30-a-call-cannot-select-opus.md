---
id: M-30
title: Let a call select Opus, or stop shipping a codec nothing can reach
pillar: Media
status: ready
priority: 4
design: docs/designs/media.md
epic: media
areas: [sipx-call, sipx-media, sipx-sdp]
note: M-13 built the codec, not the selection — sipx-call hardcodes G.711 at six sites and Codec::from_payload_type deliberately never returns Opus, so X-33 demoted RFC 6716 and 7587 to partial
---

# Let a call select Opus, or stop shipping a codec nothing can reach

## Goal
Make the Opus codec reachable from a call. `M-13` is `done` and built the encoder, the decoder and the
SDP half; nothing built the **selection**, so no call has ever carried an Opus packet.

## Acceptance
- [ ] **A call can offer and answer Opus.** Four locks have to come off, and all four are verified:
      1. `sipx-call` hardcodes `Capabilities::g711` at `call.rs:606, 752, 955, 1728, 2860, 3161`, so
         payload type 111 is never offered.
      2. `Codec::from_payload_type` (`sipx-media/src/session.rs:115-124`) **deliberately** never
         returns Opus — there is a comment saying so — so even a hand-written peer offer cannot arrive
         at it. This is the interesting one: it is a closed door, not an unfinished wire.
      3. `Capabilities::with_opus` (`sipx-sdp/src/answer.rs:85`) has no caller outside `sipx-sdp`'s own
         tests.
      4. No `sipx-call` entry point accepts caller-supplied `Capabilities` — not `dial`, `dial_early`,
         `answer`, `answer_ringing`, `answer_early`, nor any of `DialOptions`' builders — and
         `crates/sipx-call/Cargo.toml` has no `[features]` block at all, so the `opus` feature cannot
         even be turned on through it.
- [ ] **Which codec a call offers is the application's choice, with a stated default.** The default
      stays G.711: it is mandatory-to-implement and needs no C library. Opus links one and is off by
      default, so it cannot become the default by accident.
- [ ] **RFC 6716 and 7587 go back to `implemented` in the same commit that makes them true**, and the
      published table's counts move with it. `X-33` demoted both to `partial` because nothing could
      reach them; the demotion is the honest state until this closes, and reversing it without the
      code would be the exact defect that check exists to catch.
- [ ] **The public docs move in the same commit too.** `X-35` scoped every Opus mention to the crates
      — `README.md`, `website/docs/intro.md`, `does-this-fit.md`, `website/src/pages/index.js` — and
      `check-audio-claims.py` now holds all 44 front doors to agreement. When a call can select Opus
      those sentences become under-claims, and the guard will not catch an under-claim.
- [ ] Failing-first test: a call placed with Opus selected offers payload type 111 and carries Opus
      packets. It cannot pass today because there is no selector. Name it.

## Progress
- Not started. Filed at `X-33`'s integration, from its explicit request: *"it wants a story, and so
  does wiring Opus to a call — `M-13` is `done` and built the codec, not the selection."*

## Notes
- **This is the sixth instance of the project's recurring defect**, after ICE (`M-27`), UPDATE
  (`S-22`), DTLS-SRTP (`M-28`), the SDES answer check (`M-29`) and RFC 8122 — a capability
  implemented and tested inside one crate that nothing above it can select, reading as shipped.
- **It is the first one found in the public docs rather than in the registry.** Two independent
  read-only sweeps found Opus advertised on the README, in `intro.md`, in `does-this-fit.md`'s *"It
  fits if you want to"* list and on the landing page. The registry rows said `implemented` and escaped
  `X-30`'s check because they carry no `roles` field at all — which is the hole `X-33` then closed.
- **What makes this case sharper than the others**: `Codec::from_payload_type` refuses Opus with a
  comment explaining the refusal. The other five were unfinished wiring; this was a door someone shut
  on purpose while the front page advertised it as open. Read that comment before changing it — the
  reason may still be good, in which case the honest answer is to remove the codec's claims rather
  than the door.
- Compare `M-28`: the same "reachable from no call" shape, but that one is blocked on a genuine
  ordering problem (`establish` runs before the ACK). This one has no such obstacle recorded — which
  means either it is simpler than it looks, or the obstacle has not been found yet.
- Priority 4, above `M-28`'s 5: G.711-only is a real limitation for anyone on a lossy network, the
  codec is already written and tested, and the missing piece is a selector rather than a protocol
  exchange.
