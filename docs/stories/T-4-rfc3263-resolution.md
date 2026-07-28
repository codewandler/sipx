---
id: T-4
title: Implement RFC 3263 target resolution
pillar: Signalling
status: done
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
- [x] NAPTR → SRV → A/AAAA resolution with the correct precedence, weight and priority
      handling (RFC 3263 §4.1–4.3, RFC 2782 weighted selection).
- [x] Explicit port or IP literal in the URI short-circuits resolution, as the RFC requires.
- [x] `sips:` restricts candidates to TLS-capable transports.
- [x] Failure of one candidate falls through to the next; the candidate list is exhausted
      before the request fails.
- [x] Resolution is injectable so tests use a fixture resolver — no test touches real DNS.
- [x] Failing-first test: `srv_weighted_selection_matches_rfc2782_distribution`, over a fixed
      seed.

## Progress
- Done. `crates/sipx-transport/src/resolve.rs`, plus `Handle::send_to_uri` for the fallthrough.
- Both DNS and the randomness are behind traits. The RNG being injectable is what makes the
  RFC 2782 distribution assertable: over 4000 seeded draws a weight of 10 against 90 wins
  about a tenth of the time. Without a seeded RNG the test could only assert that selection
  did *something*.
- `sips:` yields no candidate at all when TLS is unavailable, rather than falling back. A
  downgrade would defeat exactly what the scheme asked for, so "no route" is the correct
  answer.
- **Gap, deliberately left:** there is no DNS client behind the trait. Every selection rule is
  implemented and tested, but the only `Resolver` implementations are test fixtures, so a URI
  naming a domain resolves to nothing at runtime. URIs with an IP literal or an explicit
  host:port short-circuit resolution entirely and work today, which covers the registrar and
  proxy targets M2 needs. Wiring a real resolver is a small follow-up and is filed as its own
  story rather than pretended away here.
- Worth knowing about fallthrough: a dead TCP candidate refuses the connection and is known
  bad in milliseconds, but a dead UDP candidate says nothing, and the only way to learn it is
  dead is to let the transaction time out — 32 seconds with the default constants. That is a
  property of UDP rather than of the resolver, but it makes a long candidate list over UDP
  slow to exhaust, and it is documented on the method.

## Notes
- Weighted SRV selection needs a seeded RNG for the distribution test to be deterministic.
