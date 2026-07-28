---
id: M-11
title: Bridge two calls
pillar: Media
status: done
priority: 9
design: docs/designs/media.md
epic: depth
areas: [sipx-media]
note:
---

# Bridge two calls

## Goal
Connect two calls so each hears the other — the first thing that requires more than one call at
a time.

## Acceptance
- [x] Audio is forwarded between two sessions without decoding when the codecs match, since
      transcoding a call that does not need it costs quality as well as CPU.
- [x] Codecs that differ are transcoded, and the fact is reported rather than hidden.
- [x] A bridge ends cleanly when either call does, with no leaked task or socket.
- [x] Failing-first test: `audio_played_into_one_call_is_heard_on_the_other`.

## Progress
- Done. `crates/sipx-media/src/bridge.rs`, on a raw path added to the session: `set_relay`
  makes a session hand packets on still encoded rather than decoding them, and `send_encoded`
  puts them back on the wire on the other leg's own sequence and timestamp.
- A claim I had to correct while testing it. The obvious argument for pass-through — "each
  decode and re-encode quantises again" — is **false for G.711**, whose decode is exactly
  invertible over all 256 codes. So for the codec sipx ships today, pass-through saves CPU and
  nothing else. The generational-loss argument is real for G.722 and Opus, and building the
  path now means those arrive into a bridge that already does the right thing. The test that
  claimed to prove it was vacuous for the same reason; the mechanism is asserted directly
  instead, in `a_relaying_session_hands_packets_on_without_decoding_them`.
- Four sessions in the tests, not two: a bridge is between *calls*, and with two a bridge that
  mixed its legs up would still pass.
- `Drop` aborts both directions. Without it, dropping a bridge leaves two tasks forwarding
  audio between calls nobody holds a handle to — the tasks keep the sessions alive through
  their `Arc`s, so the sockets never close.