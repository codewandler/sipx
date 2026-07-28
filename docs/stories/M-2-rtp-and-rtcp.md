---
id: M-2
title: Implement RTP and RTCP
pillar: Media
status: backlog
priority:
design: docs/designs/media.md
epic: media
areas: [sipx-rtp]
note:
---

# Implement RTP and RTCP

## Goal
Encode and decode RTP (RFC 3550) and enough RTCP to report, with sequence and timestamp
handling that survives loss and reordering.

## Acceptance
- [ ] RTP header encode/decode including CSRCs, extensions and padding; a malformed packet is
      rejected rather than misread.
- [ ] Sequence number wraparound is handled — the 16-bit counter wraps every ~20 minutes at
      50 packets per second, so this is a normal event and not an edge case.
- [ ] A jitter buffer that reorders, absorbs jitter and reports loss, with its depth
      configurable.
- [ ] RTCP sender and receiver reports are generated and parsed.
- [ ] Failing-first test: `sequence_wraparound_is_ordered_correctly`.

## Progress
- Not started.

## Notes
- Wraparound and reordering are where naive implementations lose audio; test them first.
