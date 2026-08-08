---
id: M-40
title: Decide whether video belongs in sipx
pillar: Media
status: in-progress
priority: 19
design: docs/designs/video.md
epic: video
areas: [sipx-sdp, sipx-media, sipx-call, interop, docs]
predicate:
announcement:
note: post-beta admission gate; the current vision says video is a non-goal, so no implementation precedes this decision
---

# Decide whether video belongs in sipx

## Goal

Make an evidence-backed admission decision before any video implementation enters the workspace.
The current vision deliberately optimizes sipx for telephony audio and names video as a non-goal;
this story may preserve that boundary or propose one narrow post-beta video profile, but cannot
silently change it.

## Acceptance

- [x] A design record measures the cost of one bounded send-and-receive video profile: SDP and
      offer/answer state, RTP packetization and depacketization, RTCP feedback, codec integration,
      frame timing and buffering, congestion response, resource budgets, security, packaging, and
      independent-peer test infrastructure. It identifies what can reuse the `webrtc-audio` epic
      and what would add video-specific state.
- [x] The record cites the applicable primary requirements, including RFC 3264, 3550, 4585, 5104,
      6184, 7741, 7742, 8834 and 9429, and resolves the initial codec/profile boundary without
      assuming that an encoder or decoder is free to ship merely because an RTP payload format is
      specified.
- [x] Measurements use bounded representative workloads to set explicit CPU, memory, queue,
      resolution, frame-rate and recovery budgets. The decision accounts for malformed payloads,
      decompression/resource exhaustion, packet loss, reordering, keyframe requests, midstream
      resolution changes and cancellation; it does not accept “a picture appeared” as evidence.
- [x] The project records one of two outcomes: **not admitted**, with the measured reason and the
      vision unchanged; or **admitted**, with an explicit vision change, a normative spec written
      before code, child stories, feature/package policy, and the maturity ladder in the roadmap.
      No implementation story becomes `ready` before the admitted outcome exists.
- [x] If admitted, the first public claim requires a bounded independent-peer proof in both offer
      and answer roles that checks decoded frame identity and timing under clean and impaired
      transport, plus negative codec-parameter and resource-limit cases. Browser compatibility is
      not claimed until `M-38` is complete and the video profile independently proves the combined
      audio/video session it advertises.

## Progress

Filed as post-beta exploration. Maturity is **0/5 (proposed)**: no video SDP profile, codec,
packetizer, media runtime, independent-peer proof, or public support claim exists. The existing RTP
and secure browser-audio prerequisites reduce unknowns, but they are not video evidence.

**Decided 2026-08-08: not admitted.** The record is [`docs/designs/video.md`](../designs/video.md);
the epic closes at maturity 0 and [`vision.md`](../vision.md) is unchanged. Three independent
reasons, each measured rather than asserted:

- **No demand.** The project's one demand instrument puts video "at or near zero"
  (`docs/designs/demand.md:26-30`). No reviewer, field report, comparison row or downstream consumer
  asks for it; the browser SDK contract refuses an offered video section with an automatic 488, and
  the SDP half of that refusal is already a running test.
- **Cost is a second media stack, not an increment.** The `webrtc-audio` transport shell (WSS, ICE,
  DTLS-SRTP, SRTP, RTCP-mux) and the SDP AST are reusable, but packetization, feedback, congestion,
  buffering, codec and proof are all new. Load-bearing findings: `sipx-rtp` has no MTU, fragmentation
  or aggregation in 4,504 lines; RTCP knows only types 200–203 on a flat 5 s timer, so RFC 4585
  timing and RFC 5104 codec control are new subsystems; the pacer is one frame → one packet → one
  20 ms tick, which video inverts; the delivered browser profile is normatively one media section
  (`docs/specs/webrtc-audio.md:88-93`), so video invalidates it rather than extending it; and
  `Codec`, `MediaProfile`, `Capabilities`, `Codecs` and `Call::media()` all break.
- **The release gate says not now.** v1 predicate 3 (`docs/roadmap.md:685`) — the public API has not
  been used from outside this repository — and predicate 4, for which this refusal is the instance.

The decisive technical finding is the codec boundary: RFC 6184 and RFC 7741 specify payload formats,
not codecs, and every practical encoder/decoder means FFI (`unsafe`, forbidden workspace-wide) or a
large hostile-input decoder. There is therefore **no initial codec**, so no profile can be scoped.

Reversal triggers, budgets and the evidence standard an admitted profile would have to meet are in
the record's *What would reverse this*, *Budgets* and *Evidence standard* sections. The last
Acceptance row is conditional on admission; it is satisfied by recording that standard in advance
rather than by a proof, because there is nothing to prove.

Left `in-progress` deliberately: closing it is `/track:done`, which also owns `CHANGELOG.md`, the
board and a `scripts/maturity.py` regeneration (the open-story counts in `docs/maturity.md` change
when this goes `done`).

## Notes

- This story is not an alpha, beta, or stable-1.0 predicate. It must not delay `1.0.0-beta.1` or
  widen that release's announcement.
- `M-38` remains audio-only. Completing it may supply WSS, ICE, DTLS-SRTP, RTP/SAVPF and RTCP-mux
  composition, but does not decide codec, feedback, buffering, congestion or resource policy for
  video.
- A decision to keep video out is a valid completed outcome. The epic exists to make the boundary
  deliberate and measurable, not to pre-approve a feature.
