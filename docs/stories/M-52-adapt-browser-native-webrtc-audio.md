---
id: M-52
title: Adapt browser-native WebRTC audio
pillar: Media
status: backlog
priority: 16
design: docs/designs/browser-sdk.md
epic: browser-sdk
areas: [browser, webrtc, audio, opus, m15]
predicate:
announcement:
note: after A-16 · reuse beta.4 profile through RTCPeerConnection, do not implement WebRTC in WASM
---

# Adapt browser-native WebRTC audio

## Goal

Map sipx's delivered fail-closed browser-audio profile onto `RTCPeerConnection` and browser media
tracks while keeping the browser, not Rust, responsible for WebRTC transport and rendering.

## Acceptance

- [ ] The adapter exchanges offers, answers and trickled ICE candidates between the session kernel
      and one audio-only `RTCPeerConnection` without parsing SDP through ad-hoc string replacements.
- [ ] Opus is preferred, G.711 and telephone events follow the delivered profile, and the selected
      codec, ICE pair, DTLS role, fingerprint and SRTP state are reported as typed facts.
- [ ] Missing RTCP multiplexing, wrong fingerprint, weaker security, video media sections, data
      channels and unsupported bundled layouts fail closed before a call is presented as connected.
- [ ] Permission denial, device removal, autoplay refusal and track end surface separately from SIP
      failure and cannot leak a live track or peer connection.
- [ ] Hangup, page teardown and aborted setup close every transceiver, track, data source and timer;
      bounded fake-media tests observe cleanup rather than waiting a fixed sleep.
- [ ] Non-silent Opus audio works in both SIP roles in the supported browser fixture and the gate is
      green.

## Progress

- Backlog. Depends on A-16 and reuses, rather than reimplements, the beta.4 browser-audio profile.
