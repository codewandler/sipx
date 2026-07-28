---
id: T-5
title: Wire a real DNS client behind the resolver trait
pillar: Signalling
status: backlog
priority:
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport]
note: T-4 implements every selection rule but has no DNS backend
---

# Wire a real DNS client behind the resolver trait

## Goal
Give `Resolver` a real implementation so a URI naming a domain resolves at runtime, not only
in tests.

## Acceptance
- [ ] An async DNS client implements `Resolver` for NAPTR, SRV and A/AAAA.
- [ ] Lookups are cached with respect for record TTLs; a stale entry is never preferred over a
      fresh lookup.
- [ ] A lookup failure is distinguishable from an empty answer — "the server is down" and
      "there is no such record" lead to different retry behaviour.
- [ ] Resolution never blocks the endpoint loop.
- [ ] Failing-first test: `a_domain_uri_resolves_through_the_real_client`, against a fixture
      DNS server rather than the public internet.

## Progress
- Not started. `T-4` left this gap explicitly: the selection logic is complete and tested, but
  the only `Resolver` implementations are fixtures.

## Notes
- URIs with an IP literal or explicit host:port short-circuit resolution and already work, so
  this does not block the first interop milestone.
