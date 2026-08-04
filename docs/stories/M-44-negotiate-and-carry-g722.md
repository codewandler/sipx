---
id: M-44
title: Negotiate and carry G.722
pillar: Media
status: backlog
priority: 16
design: docs/designs/demand.md
epic: demand
areas: [sipx-audio, sipx-sdp, sipx-rtp]
predicate:
announcement:
note: the only codec with real demand that sipx lacks · static PT 9 with the RFC 3551 clock-rate trap
---

# Negotiate and carry G.722

## Goal

Add G.722 as a negotiable codec, correctly handling the static payload type and the clock-rate
inconsistency the RFC preserves for historical reasons.

## Acceptance

- [ ] G.722 encode and decode, verified against reference vectors rather than round-trip alone.
- [ ] **The RFC 3551 §4.5.2 trap is handled explicitly:** G.722 is sampled at 16 kHz but its RTP
      timestamp clock rate is 8000. A failing-first test asserts the RTP timestamps advance at 8000
      while the audio is 16 kHz, because getting this wrong produces audio that plays at the wrong
      speed and nothing else catches it.
- [ ] G.722 is accepted as **static payload type 9 with no `a=rtpmap` line present** — the field-
      reported failure is a stack rejecting exactly that offer — and is also accepted when an
      `a=rtpmap` is supplied.
- [ ] Codec preference ordering places it correctly relative to Opus and G.711, and the choice is
      stated in the offer/answer documentation rather than implied by list order.
- [ ] The codec is selectable from the CLI (`--codec`) and appears in the audio claims that
      `scripts/check-audio-claims.py` verifies, with `crates/sipx-audio/src/lib.rs` updated in the
      same commit.
- [ ] `docs/rfc/registry.toml` RFC 3551 row updated in the same commit — it currently records
      PT 0 and 8 only; `rfc-report.py --check` green.
- [ ] `./scripts/gate.py` green.

## Progress
- (not started)

## Notes
- The one genuine codec gap in the demand survey. G.729, AMR, iLBC, GSM, Speex and T.38 drew
  essentially zero requests and are **not** in scope here or elsewhere.
- Whether to implement or bind a codec library is an implementation choice; if a C dependency is
  used it follows the Opus precedent — off by default, feature-gated, with the justification written
  down, since `unsafe_code = "forbid"` does not reach a dependency.
