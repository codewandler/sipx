---
id: M-39
title: Close the packaged and independent-peer Opus proof
pillar: Media
status: done
priority: 1
design: docs/designs/opus.md
epic: opus
areas: [sipx-audio, sipx-media, sipx-call, sipx-cli, interop]
predicate: [4, 6]
announcement: [2, 4, 5]
note: Opus is rate- and direction-correct through the CLI, normalized packages and an independent peer
---

# Close the packaged and independent-peer Opus proof

## Goal

Finish Opus as a downstream-usable product capability by preserving the completed codec and call
work while closing the diagnostic-phone audio, isolated package-feature, and independently
implemented peer gaps.

## Acceptance

- [x] RFC 6716 encode/decode and RFC 7587 dynamic payload negotiation are implemented with positive
      48 kHz audio evidence (`M-13`); SDP and media share one format identity (`M-31`); and
      codec-construction failure cannot put G.711 under an Opus payload type (`M-37`).
- [x] Both call roles can explicitly select Opus without changing the G.711 default; re-INVITE and
      early-dialog negotiation retain that selection, and feature absence fails before network I/O
      (`M-30`).
- [x] Diagnostic-phone Opus media is rate- and direction-correct. Input policy is specified rather
      than silently reinterpreting 8 kHz samples as 48 kHz; packet sizing follows the negotiated
      media clock in both `dial` and `answer`; recordings carry the correct sample-rate header; and
      G.711's existing 8 kHz behavior is unchanged.
- [x] A failing-first two-process case carries distinguishable signals in both directions and asserts
      rate, duration and signal identity, not merely a negotiated name or non-empty recording.
- [x] The feature/package boundary is complete: `sipx-audio`, `sipx-media`, `sipx-call`, and
      `sipx-cli` build in meaningful feature-off and Opus-only combinations; a clean consumer of the
      packaged manifests builds and runs the CLI with `--features opus`; the native dependency,
      licence/advisory policy and off-by-default behavior remain visible.
- [x] A bounded independent-peer case negotiates Opus in both offer/answer roles and proves audio
      encoded by each implementation is decoded by the other. Payload type, 48 kHz RTP clock and
      non-silent recovered audio are asserted; a G.711 exchange cannot satisfy the case.
- [x] The RFC registry and public codec/feature documentation cite that package and independent-peer
      evidence without claiming optional RFC 7587 `fmtp` parameters that remain unsupported.

## Progress

Done. Both command roles now validate WAV input against the negotiated media clock before queuing a
sample, use the session's packet size, and write recordings with that clock. A two-process case
carries distinct one-second 48 kHz signals in both directions and asserts duration and spectral
identity; the existing G.711 proof retains its 8 kHz contract. The feature gate exercises CLI
feature-off, Opus-only and Opus-plus-device builds, then constructs normalized local archives and
runs the packaged CLI with Opus in an isolated consumer. The release verifier requires the same
feature from exact registry bytes after publication. Finally, an Opus-only independent peer passed
both SIP roles with dynamic payload type, 48 kHz RTP time, non-silence and signal-correlation checks,
so a G.711 fallback cannot satisfy the proof. The full 30-step gate passed on the combined tree.

## Notes

- Do not rebuild the codec or copy completed positive tests into this story. Its purpose is to make
  downstream packaging and independent implementation agreement observable.
- Common optional Opus `fmtp` controls may be filed separately when a consumer needs them; absence is
  already stated in the RFC 7587 registry row and does not silently become part of this exit.
