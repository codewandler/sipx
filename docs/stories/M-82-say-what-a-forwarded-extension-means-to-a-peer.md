---
id: M-82
title: Say what a forwarded extension means to a peer that negotiated none
pillar: Media
status: backlog
priority: 20
design:
epic: media-interoperability
areas: [sipx-media, sipx-sdp]
predicate:
announcement:
note: sipx negotiates no extmap and now forwards element identifiers verbatim across a bridge
---

# Say what a forwarded extension means to a peer that negotiated none

## Goal

Resolve what a bridged RTP header extension means when neither leg negotiated `a=extmap`, either by
negotiating it or by recording why forwarding to a non-negotiating peer is safe.

## Why

`M-79` made a bridge forward the header extension it received. sipx negotiates no `extmap`, so the
bridge forwards element identifiers verbatim and cannot detect a peer that numbers them differently
— the identifiers are session-scoped by RFC 8285, so the same byte can mean two different things on
the two legs. The bridge module doc names this as the thing to revisit when `extmap` lands.

RFC 3550 §5.3.1 makes an unrecognised extension ignorable by a compliant receiver, which is the
argument for it being harmless today. That argument should be written down and checked rather than
inferred, particularly since browser offers carrying `a=extmap:` already appear in
`docs/specs/webrtc-audio.md`.

## Acceptance

- [ ] Either `a=extmap` is negotiated and the bridge maps identifiers between the two legs, or the
      decision not to is recorded in the media spec with the RFC 3550 §5.3.1 argument stated and its
      limits named.
- [ ] A test covers the two legs disagreeing about an identifier, and asserts whatever the decision
      promises.
- [ ] The browser profile's position is stated: an offer carrying `a=extmap:` is answered
      consistently with the decision, rather than by silence.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from the independent review of `M-79`'s diff.
