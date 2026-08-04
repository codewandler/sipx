---
id: T-30
title: Export signalling capture and media quality to a collector
pillar: Transport
status: backlog
priority: 17
design: docs/designs/demand.md
epic: demand
areas: [sipx-transport, sipx-rtp]
predicate:
announcement:
note: the two observability asks people maintained private forks for · hooks, not a bundled exporter
---

# Export signalling capture and media quality to a collector

## Goal

Make sipx's existing capture and RTCP data reachable by the tools operators already run, through
hooks they can wire up rather than an exporter sipx chooses for them.

## Acceptance

- [ ] Signalling capture can be encoded in the HEP3 format and sent to a configured collector,
      reusing the existing `crates/sipx-transport/src/capture.rs` path rather than duplicating it.
- [ ] **Redaction still applies.** Secrets that would still be valid are redacted before anything
      leaves the process, proven by a test — an export path that bypasses redaction is a
      credential leak with a feature name.
- [ ] Export is off by default and its failure is isolated: an unreachable collector degrades to a
      counted, logged drop and never blocks or fails a call.
- [ ] A callback exposes RTCP receiver-report data — loss, jitter, round-trip — per stream, so an
      application can feed its own metrics system.
- [ ] The callback survives a re-INVITE and an ICE restart; a reported failure against a comparable
      stack is monitoring silently dying after renegotiation, so a test covers exactly that.
- [ ] **No metrics backend is bundled.** sipx emits; the application exports. The reasoning is
      recorded, since the requests arrived attached to one specific backend.
- [ ] Documented in a guide, with the redaction guarantee stated where the export is described.
- [ ] `./scripts/gate.py` green.

## Progress
- (not started)

## Notes
- Small demand by count but unusually high quality: every requester in the survey arrived with a
  working private fork, which is the strongest signal a capability is missing rather than merely
  wished for.
- CDR and OpenTelemetry drew **zero** mentions and are deliberately not in scope.
- The RTCP data already exists in `sipx-rtp`; this is largely a reachability story, the same shape
  as `S-35` and `C-6`.
