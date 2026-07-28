# Design: Call framework

**Status:** outline · **Pillar:** Application · **Epic:** `call` · **Stories:** _to be cut_

## Why

This is the layer applications actually program against, so it is the one that decides whether
sipx is pleasant to use. It is also where the ownership principle gets its real test: bridging
two calls is precisely the situation that tempts an implementation into a shared, lock-guarded
media session.

## Approach

_To be written when the epic starts. In outline: a `Call` owns its dialog and its media
pipeline outright. Playback, recording, echo and DTMF are operations on that owned pipeline.
Bridging moves frames between calls over channels, so a stalled leg cannot block its peer;
mixing is a task that owns N receivers and one mixed output. Transfer follows RFC 3515 with
`NOTIFY` progress reporting._

## Alternatives considered

- **A shared media session behind a mutex**, with each leg holding a reference. Rejected on
  principle 3: one slow or stalled leg then stalls the other, and the failure mode is a
  latency spike nobody can reproduce.

## Risks & open questions

- Channel capacity and what to do when a bridge's receiver falls behind — dropping audio is
  correct, but only if it is measured and reported.
- Whether early media and 183 belong here or in the UA layer.

## Acceptance / done

A bridged call passes audio and DTMF in both directions, a mixed conference of three legs is
intelligible, and no media path shares mutable state between calls.
