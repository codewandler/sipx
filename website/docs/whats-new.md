---
title: What's new
description: Release highlights for sipx 1.0.0-alpha and guidance on the newer main-branch documentation.
---

# What's new

<!-- BEGIN generated:release-heading -->
## 1.0.0-alpha — 2026-07-30
<!-- END generated:release-heading -->

This is the current tagged release. It establishes a measured alpha baseline for the SIP,
transport, call, and media stack; it is not an API-stability promise. Breaking API changes are
still possible before 1.0.

Install this exact release with:

```bash
cargo install --git https://github.com/codewandler/sipx \
  --tag v1.0.0-alpha --locked sipx-cli
```

### Release highlights

- **Media startup is transactional.** A media session or conference is returned only after its
  configuration and codecs have been validated. Startup failures are typed errors, and no worker
  or socket is left behind.
- **Transport resource limits cover live work.** Connection eviction now terminates the evicted
  connection, and unauthenticated TLS and WebSocket handshakes share a finite per-endpoint budget
  and deadline.
- **Invalid runtime settings fail before binding.** Zero channel capacities, connection or
  handshake limits, handshake deadlines, WebSocket keepalives, and media worker intervals are
  rejected as configuration errors rather than reaching a panic or dead worker.
- **Conference shutdown owns its workers.** Removing a participant, closing a conference, or
  dropping it initiates cleanup of participant collectors, media sessions, and sockets.
- **Codec construction cannot change the negotiated codec.** An Opus setup failure is reported
  instead of substituting G.711 bytes under the negotiated Opus payload type.
- **Media statistics are observational.** Reading current statistics no longer resets the RTCP
  reporting interval; sending a report is the operation that closes that interval.
- **CLI recordings retain received audio.** `sipx answer` and `sipx dial` wait separately for the
  first frame and for later idle, and a duration limit preserves samples recorded before it fires.
- **RFC support claims require implementation evidence.** Implemented and partial entries in the
  generated compliance table cite workspace source, so prose alone cannot make a support claim.

The alpha also includes the shipped user-agent surface described throughout this site: calls,
registration, G.711 audio, optional Opus, RTP/RTCP, secure library transports, SDES-keyed SRTP,
and the scriptable WAV-based CLI.

### Not complete in this release

ICE and a DTLS-keyed call path are not available. The CLI cannot select TLS or secure WebSocket,
and it uses WAV files instead of sound devices. The experimental `sipx-host` process can bind and
answer calls, but application callback bindings are not implemented.

## Changes on `main`

This website is built from `main`, so a page or API link may describe work newer than the tagged
alpha. Use the tag above when reproducibility matters, and consult the
[complete changelog](https://github.com/codewandler/sipx/blob/main/CHANGELOG.md) before updating a
Git revision. Unreleased behavior is not part of `1.0.0-alpha` merely because it appears on this
site.
