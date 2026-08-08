---
id: T-30
title: Export signalling capture and media quality to a collector
pillar: Transport
status: done
priority:
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

- [x] Signalling capture can be encoded in the HEP3 format and sent to a configured collector,
      reusing the existing `crates/sipx-transport/src/capture.rs` path rather than duplicating it.
- [x] **Redaction still applies.** Secrets that would still be valid are redacted before anything
      leaves the process, proven by a test — an export path that bypasses redaction is a
      credential leak with a feature name.
- [x] Export is off by default and its failure is isolated: an unreachable collector degrades to a
      counted, logged drop and never blocks or fails a call.
- [x] A callback exposes RTCP receiver-report data — loss, jitter, round-trip — per stream, so an
      application can feed its own metrics system.
- [x] The callback survives a re-INVITE and an ICE restart; a reported failure against a comparable
      stack is monitoring silently dying after renegotiation, so a test covers exactly that.
- [x] **No metrics backend is bundled.** sipx emits; the application exports. The reasoning is
      recorded, since the requests arrived attached to one specific backend.
- [x] Documented in a guide, with the redaction guarantee stated where the export is described.
- [x] `./scripts/gate.py` green.

## Progress
- 2026-08-05: selected in the post-beta.7 transport operations wave. Redaction, bounded failure
  isolation and renegotiation-survival tests precede exporter and callback implementation.
- 2026-08-05: `docs/specs/observability-export.md` fixes the HEP3 byte subset, the existing
  capture/redaction/queue boundary, failure counters, RTCP sample arithmetic and hook ownership
  across media replacement before implementation.
- 2026-08-05: implemented byte-exact HEP3 export on the bounded capture writer with mandatory
  redaction and separate success/drop counts; added application-owned RTCP quality hooks with panic
  isolation and explicit carry-over through media replacement; documented both public paths.
  Focused HEP/redaction/failure, RTCP arithmetic/panic, re-INVITE and ICE-restart tests pass;
  formatting and targeted all-feature clippy for `sipx-transport`, `sipx-media` and `sipx-call` are
  green. The story remains `in-progress` and the gate acceptance stays open because this wave did
  not run `./scripts/gate.py` by instruction.

## Notes
- Small demand by count but unusually high quality: every requester in the survey arrived with a
  working private fork, which is the strongest signal a capability is missing rather than merely
  wished for.
- CDR and OpenTelemetry drew **zero** mentions and are deliberately not in scope.
- The RTCP data already exists in `sipx-rtp`; this is largely a reachability story, the same shape
  as `S-35` and `C-6`.
