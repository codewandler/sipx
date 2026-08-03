# Design: Call framework

**Status:** active · **Pillar:** Application · **Epic:** `call` · **Stories:** `C-2`

## Why

This is the layer applications actually program against, so it is the one that decides whether
sipx is pleasant to use. It is also where the ownership principle gets its real test: bridging
two calls is precisely the situation that tempts an implementation into a shared, lock-guarded
media session.

## Approach

A `Call` owns its dialog and its media pipeline outright. Before the final response, that same
ownership sits in the handle for the early dialog: `Ringing` on the answering side and `Dialing`
on the calling side. A reliable provisional which completes offer/answer starts the pipeline in
that owner. Confirming the dialog moves the running pipeline into `Call`; it does not stop it,
bind again, or derive its keys again. The detailed state and wire contract is
[`docs/specs/call-early-media.md`](../specs/call-early-media.md).

Playback, recording, echo and DTMF are operations on the owned pipeline. Bridging moves frames
between calls over channels, so a stalled leg cannot block its peer; mixing is a task that owns N
receivers and one mixed output. Transfer follows RFC 3515 with `NOTIFY` progress reporting.

## Alternatives considered

- **A shared media session behind a mutex**, with each leg holding a reference. Rejected on
  principle 3: one slow or stalled leg then stalls the other, and the failure mode is a
  latency spike nobody can reproduce.

## Risks & open questions

- Channel capacity and what to do when a bridge's receiver falls behind — dropping audio is
  correct, but only if it is measured and reported.
- Whether early media and 183 belong here or in the UA layer. `C-2` (M9) settles it: here, because
  an early media stream becomes the confirmed one on the 2xx without being rebuilt, and only the
  layer that owns the pipeline can do that without a gap in the audio.

## Acceptance / done

A bridged call passes audio and DTMF in both directions, a mixed conference of three legs is
intelligible, and no media path shares mutable state between calls.
