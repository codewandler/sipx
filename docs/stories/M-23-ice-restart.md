---
id: M-23
title: Recognise and act on an ICE restart
pillar: Media
status: done
priority: 3
design: docs/designs/media.md
epic: ice
areas: [sipx-media, sipx-call]
note: ice · RFC 8839 §4.4.1.1.1 · after M-22 · a restart that goes silent is worse than none
---

# Recognise and act on an ICE restart

## Goal
Survive a mid-call address change: a re-offer whose `ice-ufrag` and `ice-pwd` both changed
starts a new ICE session while the old pair keeps carrying audio.

## Acceptance
- [x] **Both** `ice-ufrag` and `ice-pwd` changed is a restart (RFC 8839 §4.4.1.1.1); one alone is not,
      and the same value moving between session and media level is explicitly not. Asked in exactly
      one place per side: `Agent::remote_description` for the agent's own rebuild, and
      `Call::peer_ice_restarted` for whether the answer needs new credentials. Both compare the
      *effective* value for the stream, which `sipx_media::ice::negotiate` resolves — so a value
      moving between session and media level is not a change.
- [x] A restart regenerates the tiebreaker, re-gathers, rebuilds the checklists, and may redetermine
      the role. `Input::LocalCredentials` carries the new credentials and tiebreaker and is applied
      **before** the description that triggers the rebuild, so the new checklists are keyed to what
      this side is about to signal rather than to the finished session's.
- [x] Media keeps flowing on the previously selected pair until the new session selects one.
      `Agent::restart` already kept `selected` for this; nothing else in the suite would notice if
      it stopped, so the acceptance test records across the whole exchange.
- [x] `c=0.0.0.0` is not used for hold; hold stays `a=inactive`/`a=sendonly` (RFC 3264) —
      `holding_an_ice_call_re_signals_ice_and_does_not_restart_it`.
- [x] Failing-first test: `a_reoffer_that_changes_both_ufrag_and_pwd_restarts_ice_without_dropping_audio`.
      Confirmed red by neutralising the restart parameters, and the hold test confirmed red by
      neutralising `offer_ice`.

## Progress
- Not started. Cut from `M-16`'s proposed split; the Acceptance above is that proposal verbatim.
- 2026-07-30: **done.** `ice.md` §13.5 written first, then the layers bottom-up.
  - The story's Acceptance is about *recognising* a restart, but recognising one is half a rule.
    RFC 8839 §4.4's other half is that a stream doing ICE restates its half in **every** later
    description, and §6 makes their absence mean the peer has stopped — so before this story a
    re-INVITE on an ICE call (a hold, a resume, a session refresh) dropped the ICE attributes
    entirely and told the far end to fall back to symmetric RTP. That is fixed here too, and the
    hold test is what pins it.
  - The running agent had no way to be reached from signalling at all: `MediaSession` did not keep
    the driver handle, and the driver took only two inputs, both from the media path. It now takes
    a third — a description arrived — with a reply carrying what the next offer or answer must
    signal, read back from the agent rather than remembered by the call layer.
  - **A restart does not re-run the STUN transaction.** The socket belongs to the receive loop once
    the session is running, so a Binding response would arrive as an ICE datagram and be read as a
    connectivity check. The host candidates are the same sockets and the reflexive one is the same
    NAT binding, which §11's keepalives have been holding open — so what is re-signalled is
    correct, but a path that changed enough to move the reflexive address is not recovered by this.
    Recorded in `ice.md` §13.5 as a limit, and it belongs with `M-24`'s relay.
  - `clippy::struct_excessive_bools` caught a fourth `bool` on `Call` and was right to: the restart
    intent is a property of one offer, so it is an argument (`IceOffer`) and not state.

## Notes
- The spec is [`docs/specs/ice.md`](../specs/ice.md), written by `M-16` before any code. Read the
  sections its Acceptance names rather than re-deriving them from the RFCs.
- `M-16` is the tracker for this epic and stays open until every child is done.
