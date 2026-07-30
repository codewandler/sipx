---
id: M-33
title: Settle whether reading a report block consumes the reporting window
pillar: Media
status: ready
priority: 4
design: docs/designs/media.md
epic: media
areas: [sipx-media]
note: found by X-18 — `session.rs:1404-1410` and `:1364-1367` carry contradictory comments about whether `report_block()` consumes a reporting window; one is wrong, and which one decides whether `MediaSession::stats()` is safe to poll
---

# Settle whether reading a report block consumes the reporting window

## Goal
Make the reporting window's behaviour a single stated fact backed by a test, so that a caller can know
whether reading statistics is free or destructive.

## Acceptance
- [ ] **The contradiction is resolved in code, not by editing one comment to match the other.**
      `crates/sipx-media/src/session.rs:1364-1367` and `:1404-1410` disagree about whether
      `report_block()` consumes a reporting window: one says reading does not consume it, the other says
      it does and that consuming it is how `fraction_lost` is computed. Determine which the code actually
      does, then decide which it *should* do against RFC 3550 §6.4.1's definition of `fraction_lost` —
      the fraction lost since the *previous* SR/RR packet, which is a statement about intervals and
      therefore about consumption.
- [ ] **`MediaSession::stats()` is documented as safe to poll, or documented as not.** This is the
      consequence that makes the story worth doing rather than a comment cleanup. If reading consumes the
      window, then an application polling `stats()` for a dashboard silently corrupts the
      `fraction_lost` that the next RR reports to the peer — and nothing in the type or the docs warns
      it. If reading does not consume, the other comment is misleading about how the fraction is derived.
      Whichever it is, say it on `stats()` where a caller meets it.
- [ ] **A test pins the chosen semantics.** Read twice with no packets in between and assert what the
      second read returns: the same numbers if reading is non-destructive, a zeroed or distinct window if
      it is. That test is what stops the two comments from drifting apart again, since it fails whichever
      one becomes false.
- [ ] **`fraction_lost` on the wire is asserted against the interval it claims.** RFC 3550 §6.4.1 is
      normative that it covers since the previous report, so lose a known number of packets across two
      reporting intervals and assert the value in the second RR — not just that a report was emitted.
- [ ] Failing-first test: name the assertion that fails while the two comments disagree. If both readings
      currently produce the same observable behaviour, say so — that changes this story from a defect to a
      documentation fix, and it is the first thing to establish rather than assume.

## Notes
- **Found by `X-18`** while adding transport counters, and deliberately not fixed there: it is in
  `sipx-media`, it is about RTCP semantics rather than observability, and the implementor flagged that
  guessing which comment was right would have been worse than leaving both.
- **Why it is not cosmetic.** Two comments disagreeing is the symptom; the defect is that the answer
  decides whether a normal, obvious thing for an application to do — poll the media session for
  statistics — degrades the protocol's own reporting. That failure would appear as a peer complaining
  about loss that did not happen, with nothing in sipx's own logs to connect it to a dashboard poll.
- **The precedent for taking a stale comment seriously is `check-pool-key.py`.** `AGENTS.md` records that
  the `ConnectionKey` field list was prose in three specs and wrong in one of them through two changes to
  the type, and nobody was told, because nothing connected the sentence to the field. This is the same
  shape one layer up: nothing connects either comment to the behaviour, so both can be wrong at once.
- Reads with `M-32` (the other media honesty gap, filed from the same story) and `M-31`.
