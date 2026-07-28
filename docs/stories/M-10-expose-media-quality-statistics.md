---
id: M-10
title: Expose media quality statistics
pillar: Media
status: done
priority: 8
design: docs/designs/media.md
epic: depth
areas: [sipx-media]
note:
---

# Expose media quality statistics

## Goal
Make a call's quality readable while it is running, from the library and from the CLI.

## Acceptance
- [x] Loss, jitter, round-trip time and MOS estimate are readable mid-call.
- [x] Round-trip time is computed from the RTCP report round trip (RFC 3550 §6.4.1), not
      guessed from anything else.
- [x] `sipx dial --stats` reports them on exit, in both output formats.
- [x] Failing-first test: `statistics_report_the_loss_that_was_actually_injected`.

## Progress
- Done. `sipx_rtp::quality` for the arithmetic, `MediaSession::quality()` for the live figure,
  `sipx dial --stats` for the report — in both output formats, with the round trip *omitted*
  rather than reported as zero when there is nothing to compute it from.
- Round-trip time needed two things sipx did not have. It never **bound a control port**, so
  it sent RTCP and could not receive any — half a control protocol, able to say what it heard
  and never to learn what the far end heard. And it only ever sent **receiver** reports, which
  carry no NTP timestamp for the far end to echo, so no peer could have told us the round trip
  even if we had been listening. Both fixed: `MediaPort` binds the pair (RFC 3550 §11), and a
  session that has sent anything sends a sender report.
- A bug caught while writing the tests: `quality()` first read `report_block()`, which
  *consumes* a reporting window — so an application polling a live quality display would have
  quietly emptied the window the next RTCP report was going to describe, and the far end would
  have been told a lossy call was clean. `polling_the_quality_does_not_empty_the_report_window`
  fails against that version.
- Loss is reported over the whole call rather than per report interval. The per-interval
  fraction is the right number to *send* and the wrong one to *show*: it swings with each
  interval, and an application sampling it sees whichever one it caught.
- The MOS is documented as an estimate and rendered to two decimal places, because the E-model
  behind it has simplified impairment terms and eight digits would invite comparing two calls
  on the last one.