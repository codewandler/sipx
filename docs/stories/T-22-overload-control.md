---
id: T-22
title: Implement overload control
pillar: Signalling
status: done
priority: 14
design: docs/designs/sip-transport.md
epic: conformance
areas: [sipx-transport, sipx-sip]
note: M11 · RFC 7339 + 7415 · something better than answering 503
---

# Implement overload control

## Goal
Let a loaded endpoint tell its upstream neighbour how much to send, and let sipx obey the same
signal from downstream — instead of the 503-and-hope that `T-19` will otherwise leave in place.

## Acceptance

**The signal (RFC 7339)**
- [x] The four `Via` header field parameters of §4 are parsed and generated with the right party
      setting each: a client adds `oc` without a value and `oc-algo` with its algorithm list (§4.1,
      §4.2 — "MUST add an 'oc-algo' parameter […] with a default value of 'loss'"); a server adds a
      value to `oc`, and adds `oc-validity` (§4.3) and `oc-seq` (§4.4), both of which "MUST NOT be
      inserted by the SIP client".
- [x] `oc-validity` defaults to 500 milliseconds when absent (§4.3), and a value of **0** means the
      server is stopping overload control, so the client "SHOULD disregard the value in the 'oc'
      parameter" (§5.7). Treating 0 as "reduce by nothing" and treating it as "control is off" differ
      the moment the server starts again.
- [x] `oc-seq` is used to discard a stale report: responses arrive out of order, and an older `oc`
      applied after a newer one silently undoes the newer one.
- [x] As a client, sipx honours what it is told: "The SIP client MUST NOT forward more requests to a
      SIP server than allowed by the current 'oc' and 'oc-algo' parameter values" (§5.5).
- [x] The loss-based algorithm (§7.2) is implemented as the default, with the two message categories
      and random discard, and the reduction is asserted against a seeded distribution rather than
      eyeballed — the same discipline `T-4` used for RFC 2782's weighted shuffle.
- [x] Prioritisation is possible, not hardcoded: §5.10.1's "A SIP client SHOULD honor the local policy
      for prioritizing SIP requests such as policies based on message type" needs a hook, so that an
      in-dialog request or an emergency call can survive a reduction that sheds new INVITEs.

**The rate half (RFC 7415)**
- [x] `rate` is offered in the `oc-algo` list (§3.3) and the `oc` value under it is a request rate per
      second rather than a loss percentage (§3.4).
- [x] Pacing conforms to "the upper bound of 1/T messages per second" (§3.5.1), with the burst
      tolerance parameter exposed rather than fixed.

**Where it lands**
- [x] The shed path is the one `T-19` builds: a request shed under overload is counted, and now also
      reported upstream through `oc` rather than only answered 503. `T-19`'s test must still pass.
- [x] The registry entries for RFC 7339 and RFC 7415 move off "not started" in the same change, and
      RFC 7339's note stops describing sipx's 503 as the current behaviour.
- [x] Failing-first test: `a_client_told_to_reduce_by_half_forwards_half_as_many_requests`.

## Progress

- Typed `oc`, `oc-algo`, `oc-validity` and `oc-seq` parameters now enforce which side may set each
  field. An endpoint opting into client advertisement offers `loss,rate` on every request;
  responses update sequenced per-IP:port state, with the 500 ms default, expiry and zero-validity
  stop semantics applied before admission. The extension is off by default since `X-63`; the server
  half still responds whenever the upstream request offered it.
- Loss admission has deterministic seeded evidence for the RFC's two categories and a configurable
  policy hook. Rate admission is a driver-time leaky bucket with a configurable tolerance in target
  intervals, including strict pacing and a zero-rate refusal.
- Local refusals are typed `Error::Overloaded` and increment `overload_rejections` before any
  transaction or network write exists. The existing T-19 queue-full path retains its 503 and shed
  count and now adds configured, sequenced feedback for an upstream peer that offered it.
- A finite `sipx_call::load::run_bounded` endpoint scenario supplies the shared P-12 evidence:
  128 attempts, at most eight active, a two-second cleanup budget, both forwarded and rejected
  work, and exact agreement between rejected outcomes and the endpoint counter.
- RFC 7339 and RFC 7415 are now `partial`; the registry names the remaining sequence-wrap and
  autonomous server-estimation gaps instead of overstating the implementation.

## Notes
- The reason this is transport work and not application work: the signal rides on the `Via` of every
  request and response, and the thing that has to obey it is the endpoint's send path. Putting it
  above the endpoint would mean every application reimplementing the arithmetic.
- The two algorithms are not alternatives to choose between once. A client offers a list and the
  server picks, so both have to exist for either to be negotiable.
- 503 does not go away. It stays as the answer when there is nothing left to shed; what changes is
  that it stops being the *only* answer, which is what makes a neighbour oscillate.
