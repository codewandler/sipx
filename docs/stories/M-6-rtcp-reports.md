---
id: M-6
title: Implement RTCP sender and receiver reports
pillar: Media
status: ready
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
- [ ] Sender and receiver reports encode and decode, including the report blocks.
- [ ] Loss fraction, cumulative loss, extended highest sequence number and interarrival jitter
      are computed per RFC 3550 §6.4.1 and §A.3 — the jitter formula is a smoothed estimate,
      not a variance, and getting it wrong reports plausible nonsense.
- [ ] Reports are sent on the odd port alongside the media, at the RFC 3550 §6.2 interval.
- [ ] A compound packet is parsed as a compound packet; a receiver report arriving alone is
      also accepted.
- [ ] Statistics are exposed on the media session so a caller can read them mid-call.
- [ ] Failing-first test: `a_receiver_report_counts_the_loss_the_buffer_saw`.

## Progress
- Not started. `M-2` implemented RTP and left RTCP explicitly undone.
