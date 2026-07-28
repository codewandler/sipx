---
id: T-17
title: Resolve at proxy throughput — async and shared-cache
pillar: Signalling
status: ready
priority: 9
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport]
note: track: reachability · the Resolver trait is shaped for one UA, not for a forwarding element
---

# Resolve at proxy throughput — async and shared-cache

## Goal
Offer an RFC 3263 resolution path that a forwarding element can use: asynchronous, with one cache
shared across callers, so resolving a destination never blocks the loop that is forwarding
everything else.

## Acceptance
- [ ] An async resolution API exists alongside the current one; a caller can await a target list
      without occupying the calling task's thread for the duration of a DNS round trip.
- [ ] The cache is shareable across callers and honours TTLs both ways: a positive answer and a
      negative one are both cached, and a negative answer is distinguishable from "could not ask".
- [ ] `_sip._ws` and `_sips._wss` are prefetched alongside `_sip._udp`, `_sip._tcp` and
      `_sips._tcp`. A WebSocket destination currently misses the prefetch and pays a serial lookup.
- [ ] The existing synchronous `Resolver` trait keeps working unchanged — a UA that resolves one
      URI per call should not have to become async to do it.
- [ ] Failing-first test: `two_concurrent_resolutions_of_one_name_make_one_query`.

## Progress
- Not started. `Resolver` is sync with per-URI prefetch — correct and sufficient for a phone.

## Notes
- Whether this lands here or stays downstream is the open question this story exists to settle:
  the cache and the SRV prefix table are protocol-generic, the scheduling policy around them may
  not be. Record the answer either way.
- Raised by [sipx-clstr](https://github.com/codewandler/sipx-clstr)'s `RT-1`
  ([ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md)), where a single
  node resolves for every call it forwards rather than for its own one call.
