---
id: X-25
title: Write the media design record the ICE stories keep citing
pillar: Build
status: ready
priority: 6
design:
epic: ice
areas: [docs]
note: found by M-16 — six stories name docs/designs/media.md as their design, and it is a stub
---

# Write the media design record the ICE stories keep citing

## Goal
Make `docs/designs/media.md` describe the media stack that exists, so the six ICE stories that name
it as their `design:` are pointing at something worth opening.

## Acceptance
- [ ] `docs/designs/media.md` covers what `M-1` … `M-20` actually built: the RTP/RTCP path and its
      jitter buffer, the codecs, symmetric-RTP address learning, DTLS-SRTP (`M-15`), the bridge and
      conference (`M-11`), playback control (`M-17`), mute (`M-18`), and where ICE now sits.
- [ ] It states the decisions a reader cannot recover from the code — chiefly why the media state
      machines are sans-IO with a driver over them, which is the pattern `docs/specs/ice.md`
      assumes without arguing for.
- [ ] It stops claiming to be an outline with stories "to be cut": the header currently says
      `Status: outline · Stories: _to be cut_`, and eighteen of them have been cut and delivered.
- [ ] The relationship to `docs/specs/ice.md` is explicit — a design record says why, a spec says
      what, and a reader arriving from a story's `design:` field should be told which they want.

## Progress
- Not started.

## Notes
- Found by `M-16` while writing the ICE spec: the design record predates `M-1` and mentions neither
  ICE, NAT traversal nor DTLS-SRTP, yet `M-16` and all six of `M-19` … `M-24` cite it, and so do
  `M-17` and `M-18`. Every one of those stories sends its implementor to a stub.
- Low urgency, real cost: a `design:` field that points at nothing trains the next implementor to
  skip the field, and the next story after that has a design worth reading.
- Sibling of `X-24` in kind — a document that fell behind the code it describes — but not in
  remedy. A design record is an argument and cannot be generated; what keeps it honest is being
  worth reading, not a check.
