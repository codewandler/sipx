---
id: M-6
title: Implement RTCP sender and receiver reports
pillar: Media
status: done
priority: 4
design: docs/designs/media.md
epic: media
areas: [sipx-rtp, sipx-media]
note: gap left explicitly by M3
---

# Implement RTCP sender and receiver reports

## Goal
Report on the media: RTCP sender and receiver reports, so a call's quality is observable and a
peer that asks gets an answer.

## Acceptance
- [x] Sender and receiver reports encode and decode, including the report blocks.
- [x] Loss fraction, cumulative loss, extended highest sequence number and interarrival jitter
      are computed per RFC 3550 §6.4.1 and §A.3 — the jitter formula is a smoothed estimate,
      not a variance, and getting it wrong reports plausible nonsense.
- [x] Reports are sent on the odd port alongside the media, at the RFC 3550 §6.2 interval.
- [x] A compound packet is parsed as a compound packet; a receiver report arriving alone is
      also accepted.
- [x] Statistics are exposed on the media session so a caller can read them mid-call.
- [x] Failing-first test: `a_receiver_report_counts_the_loss_the_buffer_saw`.

## Progress
- Done. `crates/sipx-rtp/src/rtcp.rs`, reported from the media session on the odd port.
- Interarrival jitter is the RFC's own recurrence — `J += (|D| - J) / 16` — and not a variance.
  A variance produces numbers that look plausible, move in the right direction and are wrong by
  a factor that depends on the traffic, which is worse than reporting nothing because someone
  will tune a jitter buffer with them. Two tests pin it: evenly spaced packets report exactly
  zero however large the constant clock offset, and more unevenness reports more jitter.
- The loss fraction covers the interval since the last report, not the whole call, so a call
  that lost heavily and recovered can be seen to recover. Cumulative loss is signed, because
  duplicates drive it negative.
- One decoder bug the round-trip test caught: cumulative loss lives at bytes 5–7 of a report
  block and I read from 4, folding the fraction byte into the high byte of the count — 42 lost
  became 1.7 million.
- One test expectation was wrong rather than the code: a packet lost across an interval
  boundary is attributed to the interval in which it became known lost, which is correct.
- Interval: RFC 3550 §6.2 scales with bandwidth and membership, and for a two-party call that
  arithmetic always lands on the five-second minimum, so sipx uses it directly rather than
  implementing a calculation that could only return one answer.
