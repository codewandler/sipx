---
id: M-18
title: Mute and unmute a call's outbound audio
pillar: Media
status: backlog
priority:
design: docs/designs/app-sdk.md
epic: app-sdk
areas: [sipx-media, sipx-call]
note: app-sdk · independent · size S
---

# Mute and unmute a call's outbound audio

## Goal
A local media gate: stop contributing audio to the far end without renegotiating the session and
without touching reception — distinct from hold, which is a signalled state the far end sees.

## Acceptance
- [ ] Muting a call suppresses its outbound audio at the media layer; unmuting restores it. No
      re-INVITE is sent, the SDP direction is unchanged, and the far end's hold state is
      unaffected — the difference from `reinvite(Direction::SendOnly)` is stated in the docs.
- [ ] Whether mute sends silence frames or stops sending RTP is decided in the design and
      recorded, with the RTCP consequence stated (RFC 3550 §6 — the stream's reports must remain
      truthful either way), and the chosen behaviour is tested from the receiving side.
- [ ] Reception is untouched: a muted call still receives audio and DTMF, and `recv_digit`,
      recording and quality statistics keep working while muted.
- [ ] Mute state is queryable and transitions are observable as events (`C-3`).
- [ ] Failing-first test: `a_muted_call_contributes_no_audio`.

## Progress
- Not started.

## Notes
- `mute` does not exist anywhere in the workspace today; the closest primitives are
  `Direction::Inactive`/`SendOnly` re-INVITEs (signalled, visible to the peer) or simply not
  calling `send`/`play` (which no host holding a bridged call can arrange).
- Requested by the downstream application platform (working name `sipx-app`): `mute`/`unmute`
  are contract verbs.
