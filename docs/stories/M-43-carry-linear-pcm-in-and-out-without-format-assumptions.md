---
id: M-43
title: Carry linear PCM in and out without format assumptions
pillar: Media
status: backlog
priority: 15
design: docs/designs/demand.md
epic: demand
areas: [sipx-audio, sipx-media, sipx-rtp]
predicate:
announcement:
note: four reported use cases resolve to one unopinionated PCM boundary · not an AI feature
---

# Carry linear PCM in and out without format assumptions

## Goal

Give applications a clean linear-PCM boundary — raw samples in and out at a sample rate and bit
depth they choose, with resampling to and from the negotiated codec — so a call can be bridged to
any external audio consumer without sipx assuming 8 kHz µ-law everywhere.

## Acceptance

- [ ] Playback accepts linear PCM at a stated sample rate and bit depth rather than assuming 16-bit,
      and a source whose depth or rate differs from the negotiated codec's is converted rather than
      distorted. A failing-first test plays 8-bit and 16-bit sources at two rates and asserts the
      decoded output, not merely that no error occurred.
- [ ] A capture path exposes received audio as linear PCM at a caller-chosen rate.
- [ ] Resampling exists between the supported rates. `crates/sipx-audio/src/lib.rs` currently
      documents its absence as deliberate and `scripts/check-audio-claims.py` enforces that claim —
      **both must be updated in the same commit**, and the note must say what is now supported
      rather than being deleted.
- [ ] L16 (RFC 3551 §4.5.11) is negotiable as a codec where the peer offers it, with the correct
      static payload types for 8 kHz mono and the dynamic case otherwise.
- [ ] No hardcoded bit depth or sample rate remains on the playback or record paths; a test asserts
      the failure mode reported in the field — a raw stream at an unexpected depth producing audible
      garbage — is now either correct or a typed refusal.
- [ ] The boundary is reachable from the CLI and documented in a guide.
- [ ] `docs/rfc/registry.toml` updated in the same commit if the L16 row changes;
      `rfc-report.py --check` green.
- [ ] `./scripts/gate.py` green.

## Progress
- (not started)

## Notes
- The demand survey found four separate reports that read as codec requests and resolve to this one
  capability — users bridging calls to external real-time voice services, all of which accept linear
  PCM and few of which accept µ-law. See [`docs/designs/demand.md`](../designs/demand.md).
- **Deliberately not an AI feature.** Answering-machine detection, voice activity detection and
  speech integrations drew zero requests; an unopinionated PCM boundary serves every one of the
  reported cases and composes with any provider.
- Where resampling lives — `sipx-audio` directly, or behind a feature with a dependency — is an open
  question this story decides, per the design.
