---
id: T-5
title: Wire a real DNS client behind the resolver trait
pillar: Signalling
status: done
priority: 3
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
- [x] An async DNS client implements `Resolver` for NAPTR, SRV and A/AAAA.
- [x] Lookups are cached with respect for record TTLs; a stale entry is never preferred over a
      fresh lookup.
- [x] A lookup failure is distinguishable from an empty answer — "the server is down" and
      "there is no such record" lead to different retry behaviour, and conflating them turns a
      transient outage into a permanent one.
- [x] Resolution never blocks the endpoint loop.
- [x] Failing-first test: `a_domain_uri_resolves_through_the_real_client`, against a fixture
      DNS server rather than the public internet.

## Progress
- Done. `crates/sipx-transport/src/dns.rs`, behind the `dns` feature (on by default).
- Telling a failure from an empty answer turned out to be the hard part, and not for the
  reason expected. The client reports an unreachable nameserver as `NoRecordsFound` with
  response code `NXDomain` — the same shape as a real negative answer — so the error kind
  cannot distinguish them. The signal that can is RFC 2308's: a genuine negative answer carries
  the zone's SOA, because that is what says how long to cache it. A synthesised one has none.
- The bias is deliberate and documented: a real negative from a server that omits the SOA is
  treated as "could not ask", so sipx retries rather than falling through. Retrying a name
  that does not exist costs one lookup; caching a network blip as a routing decision costs
  every call to that domain until something evicts it.
- `Prefetched` does all the awaiting up front and hands the selection logic plain data, which
  is what keeps the endpoint loop — and every transaction timer it owns — off the DNS path.
- The tests run against a fixture nameserver on localhost with a query counter, so "served
  from cache" is an assertion rather than a claim. A first draft of that test said it pointed
  at a dead server and did not; the counter replaced the claim. `T-4` left this gap explicitly: every selection rule is implemented and tested,
  but the only `Resolver` implementations are fixtures.
