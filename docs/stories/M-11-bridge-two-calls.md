---
id: M-11
title: Bridge two calls
pillar: Media
status: ready
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
- [ ] Audio is forwarded between two sessions without decoding when the codecs match, since
      transcoding a call that does not need it costs quality as well as CPU.
- [ ] Codecs that differ are transcoded, and the fact is reported rather than hidden.
- [ ] A bridge ends cleanly when either call does, with no leaked task or socket.
- [ ] Failing-first test: `audio_played_into_one_call_is_heard_on_the_other`.

## Progress
- Not started.
