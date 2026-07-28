---
id: T-4
title: Implement RFC 3263 target resolution
pillar: Signalling
status: backlog
priority:
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport]
note:
---

# Implement RFC 3263 target resolution

## Goal
Turn a SIP URI into an ordered list of transport, address and port candidates the way the RFC
says, so sipx reaches real deployments rather than only hosts with an A record.

## Acceptance
- [ ] NAPTR → SRV → A/AAAA resolution with the correct precedence, weight and priority
      handling (RFC 3263 §4.1–4.3, RFC 2782 weighted selection).
- [ ] Explicit port or IP literal in the URI short-circuits resolution, as the RFC requires.
- [ ] `sips:` restricts candidates to TLS-capable transports.
- [ ] Failure of one candidate falls through to the next; the candidate list is exhausted
      before the request fails.
- [ ] Resolution is injectable so tests use a fixture resolver — no test touches real DNS.
- [ ] Failing-first test: `srv_weighted_selection_matches_rfc2782_distribution`, over a fixed
      seed.

## Progress
- Not started.

## Notes
- Weighted SRV selection needs a seeded RNG for the distribution test to be deterministic.
