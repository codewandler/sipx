---
id: M-40
title: Decide whether video belongs in sipx
pillar: Media
status: ready
priority: 19
design:
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

- [ ] A design record measures the cost of one bounded send-and-receive video profile: SDP and
      offer/answer state, RTP packetization and depacketization, RTCP feedback, codec integration,
      frame timing and buffering, congestion response, resource budgets, security, packaging, and
      independent-peer test infrastructure. It identifies what can reuse the `webrtc-audio` epic
      and what would add video-specific state.
- [ ] The record cites the applicable primary requirements, including RFC 3264, 3550, 4585, 5104,
      6184, 7741, 7742, 8834 and 9429, and resolves the initial codec/profile boundary without
      assuming that an encoder or decoder is free to ship merely because an RTP payload format is
      specified.
- [ ] Measurements use bounded representative workloads to set explicit CPU, memory, queue,
      resolution, frame-rate and recovery budgets. The decision accounts for malformed payloads,
      decompression/resource exhaustion, packet loss, reordering, keyframe requests, midstream
      resolution changes and cancellation; it does not accept “a picture appeared” as evidence.
- [ ] The project records one of two outcomes: **not admitted**, with the measured reason and the
      vision unchanged; or **admitted**, with an explicit vision change, a normative spec written
      before code, child stories, feature/package policy, and the maturity ladder in the roadmap.
      No implementation story becomes `ready` before the admitted outcome exists.
- [ ] If admitted, the first public claim requires a bounded independent-peer proof in both offer
      and answer roles that checks decoded frame identity and timing under clean and impaired
      transport, plus negative codec-parameter and resource-limit cases. Browser compatibility is
      not claimed until `M-38` is complete and the video profile independently proves the combined
      audio/video session it advertises.

## Progress

Filed as post-beta exploration. Maturity is **0/5 (proposed)**: no video SDP profile, codec,
packetizer, media runtime, independent-peer proof, or public support claim exists. The existing RTP
and secure browser-audio prerequisites reduce unknowns, but they are not video evidence.

## Notes

- This story is not an alpha, beta, or stable-1.0 predicate. It must not delay `1.0.0-beta.1` or
  widen that release's announcement.
- `M-38` remains audio-only. Completing it may supply WSS, ICE, DTLS-SRTP, RTP/SAVPF and RTCP-mux
  composition, but does not decide codec, feedback, buffering, congestion or resource policy for
  video.
- A decision to keep video out is a valid completed outcome. The epic exists to make the boundary
  deliberate and measurable, not to pre-approve a feature.
