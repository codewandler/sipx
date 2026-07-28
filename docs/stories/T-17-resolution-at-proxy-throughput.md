---
id: T-17
title: Resolve at proxy throughput — async and shared-cache
pillar: Signalling
status: done
priority:
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport]
note: M7 · the Resolver trait is shaped for one UA, not for a forwarding element
---

# Resolve at proxy throughput — async and shared-cache

## Goal
Offer an RFC 3263 resolution path that a forwarding element can use: asynchronous, with one cache
shared across callers, so resolving a destination never blocks the loop that is forwarding
everything else.

## Acceptance
- [x] An async resolution API exists alongside the current one; a caller can await a target list
      without occupying the calling task's thread for the duration of a DNS round trip.
- [x] The cache is shareable across callers and honours TTLs both ways: a positive answer and a
      negative one are both cached, and a negative answer is distinguishable from "could not ask".
- [x] `_sip._ws` and `_sips._wss` are prefetched alongside `_sip._udp`, `_sip._tcp` and
      `_sips._tcp`. A WebSocket destination currently misses the prefetch and pays a serial lookup.
- [x] The existing synchronous `Resolver` trait keeps working unchanged — a UA that resolves one
      URI per call should not have to become async to do it.
- [x] Failing-first test: `two_concurrent_resolutions_of_one_name_make_one_query`.

## Progress
- Done, and **smaller than the story expected** — two of the four criteria turned out to be
  already satisfied, and finding that out was most of the work.
- **The single-flight layer was written, measured, and removed.** The named failing-first test
  asserts that eight concurrent lookups of one name reach the nameserver once. It passes. It also
  passed with the single-flight layer deleted: `hickory-resolver` already coalesces concurrent
  identical queries, so the layer on top of it changed nothing. Keeping it would have been
  decoration with a comment claiming otherwise. The test stays — the property is load-bearing, and
  it is now a checked fact rather than an assumption about a dependency.
- **A genuine negative answer was not being cached, and now is** (RFC 2308 §5). `classify` returned
  early, so an SOA-backed NXDOMAIN was re-queried on every call. For a UA that is one extra lookup
  per call; for a forwarding element resolving for every call it forwards, a domain with no
  `_sips._tcp` record was asked about thousands of times a minute. The negative TTL comes from the
  zone's SOA, capped like a positive one.
  - `Unavailable` is still deliberately not cached, and that is now tested too: remembering a
    network blip as a routing decision keeps a domain unreachable long after it has come back.
- `_sip._ws` and `_sips._wss` join the prefetch. The test asserts the *names asked for*, not a
  query count — a count cannot distinguish "prefetched" from "not cached", because an absent name
  with no SOA is read as "could not ask" and re-queried either way. That is the test I wrote
  wrongly the first time.
- `dns::resolve_uri` is the one-await entry point for a caller that is not the endpoint loop. The
  two-step form stays, and stays the one the loop uses — every await belongs off the loop, where a
  slow nameserver would otherwise stop the transaction timers.
- **The open question in the notes, answered: it belongs here.** The cache, the negative-TTL policy
  and the SRV prefix table are protocol-generic and now proven so by tests that do not know what is
  above them. What did *not* land here is a scheduling policy, because there was none to write: the
  concurrency the story wanted already exists in the DNS client.

## Notes
- Whether this lands here or stays downstream is the open question this story exists to settle:
  the cache and the SRV prefix table are protocol-generic, the scheduling policy around them may
  not be. Record the answer either way.
- Raised by [sipx-clstr](https://github.com/codewandler/sipx-clstr)'s `RT-1`
  ([ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md)), where a single
  node resolves for every call it forwards rather than for its own one call.
