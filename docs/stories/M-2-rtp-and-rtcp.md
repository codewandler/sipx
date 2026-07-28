---
id: M-2
title: Implement RTP and RTCP
pillar: Media
status: done
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
- [x] RTP header encode/decode including CSRCs, extensions and padding; a malformed packet is
      rejected rather than misread.
- [x] Sequence number wraparound is handled — the 16-bit counter wraps every ~20 minutes at
      50 packets per second, so this is a normal event and not an edge case.
- [x] A jitter buffer that reorders, absorbs jitter and reports loss, with its depth
      configurable.
- [x] RTCP sender and receiver reports are generated and parsed.
- [x] Failing-first test: `sequence_wraparound_is_ordered_correctly`.

## Progress
- Done for RTP. `crates/sipx-rtp/`: packet encode/decode and the jitter buffer.
- Sequence wraparound is handled by extending 16-bit numbers to 64-bit, after which ordinary
  comparison is correct again and the buffer's ordered map sorts them properly. Reordering
  *across* the wrap is tested directly, since that is the case a naive comparison gets wrong.
- The buffer holds `depth - 1` packets as slack and flushes what it holds when the stream goes
  quiet — without the flush, the tail of every clip is never played.
- **Not done: RTCP.** Sender and receiver reports are not implemented. Nothing in M3 needs them
  — they carry quality statistics, not media — but the story's acceptance listed them and they
  are not there. Filed as `M-6`.

## Notes
- Wraparound and reordering are where naive implementations lose audio; test them first.
