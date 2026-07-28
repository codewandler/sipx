---
id: T-21
title: Be reachable through a push notification
pillar: Signalling
status: backlog
priority:
design:
epic: conformance
areas: [sipx-ua, sipx-sip]
note: M10 · RFC 8599 · a client holding no connection at all
---

# Be reachable through a push notification

## Goal
Let a client that holds no connection — no socket, no keepalive, possibly not running — be woken by
its push notification service and be reachable for the call that woke it.

## Acceptance
- [ ] A REGISTER can carry the push parameters RFC 8599 §8.7 defines on its `Contact` URI:
      `pn-provider`, `pn-param` and `pn-prid`. They are URI parameters, so they belong in the URI
      grammar rather than being pasted onto a serialized contact.
- [ ] The `sip.pns` feature-capability indicator (§8.2) is understood, so a client can tell whether
      the registrar supports the push service it named — and `sip.pnsreg` and `sip.pnspurr` are at
      least parsed, since a registrar that offers them changes what the client should do next.
- [ ] **555 (Push Notification Service Not Supported)** (§8.1) is a known status code and is
      surfaced as itself, not as a generic failure. It is the one answer that tells a client its
      whole reachability plan is wrong.
- [ ] On receiving a push, the client sends a binding-refresh REGISTER — §4.1.3: "When a UA receives
      a push notification, the UA MUST send a binding-refresh REGISTER request" — and only then
      expects the pending request. A client that waits for the INVITE without refreshing has no flow
      for it to arrive on.
- [ ] The push service itself is behind a trait and sipx ships no implementation of one. sipx is a
      stack, not a client of a particular vendor's push transport; the [vision](../vision.md)'s
      non-goals rule out anything else, and a test double is what the tests use.
- [ ] `pn-purr` is handled or explicitly deferred with a reason recorded — it exists so a
      mid-dialog request can be matched to a stored binding, which only matters once something
      stores them.
- [ ] The RFC registry entry for RFC 8599 moves off "not started", with the `Roles` column saying
      the UA half only.
- [ ] Failing-first test: `a_push_wakes_a_client_that_refreshes_its_binding_before_the_invite`.

## Progress
- Not started. `compliance.md` records RFC 8599 as depending on 5626, which is `T-15` in M6.

## Notes
- The ordering in §4.1.3 is the whole mechanism and the easiest thing to get backwards. The push is
  not the call; it is permission to go and get a flow, and the INVITE arrives down the flow.
- The story is smaller than it looks precisely because sipx implements no push provider. What it
  owes is the SIP half: the parameters, the option negotiation, 555, and the refresh ordering.
- Scope: the client side. The proxy behaviour of §5.6 — holding the request in a push bucket while
  the client wakes — is a proxy role and belongs where the other proxy roles do, outside this repo.
