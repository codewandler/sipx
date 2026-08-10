---
id: C-9
title: Carry a bridge and a conference across a renegotiation
pillar: Signalling
status: in-progress
priority: 8
design: docs/designs/app-sdk.md
epic: app-sdk
areas: [sipx-call, sipx-media]
predicate:
announcement:
note: filed from C-6 · a re-INVITE, a hold/resume or an ICE restart replaces a call's MediaSession and the bridge keeps forwarding the stopped one · size S/M
---

# Carry a bridge and a conference across a renegotiation

## Goal
A [`CallBridge`] or a [`CallConference`] participation survives its call renegotiating its media,
so a host does not have to notice a re-INVITE and remake the coupling by hand.

## Acceptance
- [x] Failing-first test: two bridged calls, one of them put on hold and resumed (or otherwise
      re-INVITEd), still pass audio afterwards — asserted at the far ends, not from the session.
- [x] The same for a conference participation: a participant that renegotiates keeps being heard
      and keeps hearing the mix, without rejoining.
- [x] `CallBridge::is_connected` and `Call::is_bridged` stay truthful across the transition rather
      than reporting a bridge that is forwarding a stopped session.
- [x] No new shared mutable session: the rebind stays channel-backed and keeps the vision's
      principle 3, exactly as `C-6`'s bridge does.
- [x] The `C-6` documentation that currently states this limitation — `crates/sipx-call/src/bridge.rs`
      and `crates/sipx-call/src/conference.rs`, "What a bridge does not survive" — is removed rather
      than softened.

## Progress
- 2026-08-08: filed from `C-6`, which built the `Call`-facing bridge and conference and
  deliberately stopped short of this.
- 2026-08-10: specified vectors B1/C5 before implementation. A call now publishes a replacement
  media handle to its existing bridge and conference memberships before retiring the old session;
  bridge relay ordering is preserved and a conference collector follows a bounded watch channel
  under the same participant ID.
- 2026-08-10: the call-facing bridge suite passes all nine tests, including bidirectional far-end
  audio and truthful bridge state after a raw in-dialog re-INVITE. The media conference suite
  passes all ten tests, including bidirectional audio through a stable rebind.
- 2026-08-10: the complete local acceptance gate passed all 51 steps on the rc.23 candidate.

## Notes
- The shape of the problem: `Call` holds `media: Arc<MediaSession>` and *replaces* it on a
  renegotiation, retiring the old one (`retired_media`). `C-6`'s `bridge::Link` holds the two
  handles it was given at connect time, so after a replacement it forwards a session that has
  stopped. `Coupling::bridge_media` already faces this and answers it by simply re-bridging
  (`crates/sipx-call/src/coupling/mod.rs`), which is the cheap version of what this story wants.
- Care needed on ordering: `sipx_media::Bridge` sets the encoded-relay flag on both sessions at
  connect and deliberately does **not** clear it at close, because replacing one bridge with
  another drops the old one last. A rebind has to set relay on the *new* session and clear it on
  the retired one, in that order.
- `M-12`'s `Conference` has the same shape: `join` captures one `Arc<MediaSession>` and the
  collector ends when that session stops, leaving the member registered and contributing silence.
  Leaving and rejoining under a new id is one answer; a rebind on the participant is the other.
