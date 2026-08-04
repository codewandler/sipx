---
id: M-37
title: Never put a fallback codec under a negotiated payload type
pillar: Media
status: done
priority: 7
design: docs/designs/media-runtime-safety.md
epic: media-runtime-safety
areas: [sipx-media]
predicate: 4
note: R-08 in the 2026-07-30 repository review — failed Opus setup constructs Direct PCMU state while retaining the negotiated Opus payload type
---

# Never put a fallback codec under a negotiated payload type

## Goal

Keep media pipeline state truthful to SDP negotiation: if the negotiated codec cannot be constructed,
fail or disable that route explicitly instead of processing a different codec under its payload type.

## Acceptance

- [x] Specify codec-construction failure behavior in the media design/spec before implementation,
      including offer, answer and receive-side setup.
- [x] Opus encoder or decoder construction failure cannot produce a direct PCMU pipeline carrying an
      Opus payload type.
- [x] A required negotiated codec failure returns a typed setup error; an optional route may be
      omitted only when negotiation and observable diagnostics remain consistent with that omission.
- [x] Failing-first tests inject encoder and decoder construction failure independently and prove that
      no PCMU bytes are emitted or decoded under the Opus payload type.
- [x] Existing successful Opus and PCMU paths retain packet-level round-trip tests, including dynamic
      payload-type mapping.
- [x] Diagnostics identify codec construction failure without logging media or key material.

## Progress

- Filed from R-08 in `docs/reviews/2026-07-30T07-50-49+02-00-repository-review.md`.
- M-13 tracks successful Opus support. It has no acceptance case for construction failure, so this
  story records the distinct wire-correctness defect rather than reopening that completed feature.
- Done. `MediaPort::start` constructs the negotiated encoder and decoder before starting a worker and
  returns `SetupError::Codec` with the codec and direction on failure. The injected three-channel
  encoder and decoder cases prove no datagram is emitted and no G.711 fallback state is installed;
  the successful Opus and dynamic-payload round trips remain alongside them. The normative boundary
  is `docs/specs/media-runtime.md` and the ownership decision is in
  `docs/designs/media-runtime-safety.md`.

## Notes

- “Disabled” internal state is acceptable only if it cannot be selected as an active negotiated
  pipeline. Substituting another wire codec is not graceful degradation.
