---
id: T-21
title: Be reachable through a push notification
pillar: Signalling
status: done
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
- [x] A REGISTER can carry the push parameters RFC 8599 §8.7 defines on its `Contact` URI:
      `pn-provider`, `pn-param` and `pn-prid`. They are URI parameters, so they belong in the URI
      grammar rather than being pasted onto a serialized contact.
- [x] The `sip.pns` feature-capability indicator (§8.2) is understood, so a client can tell whether
      the registrar supports the push service it named — and `sip.pnsreg` and `sip.pnspurr` are at
      least parsed, since a registrar that offers them changes what the client should do next.
- [x] **555 (Push Notification Service Not Supported)** (§8.1) is a known status code and is
      surfaced as itself, not as a generic failure. It is the one answer that tells a client its
      whole reachability plan is wrong.
- [x] On receiving a push, the client sends a binding-refresh REGISTER — §4.1.3: "When a UA receives
      a push notification, the UA MUST send a binding-refresh REGISTER request" — and only then
      expects the pending request. A client that waits for the INVITE without refreshing has no flow
      for it to arrive on.
- [x] The push service itself is behind a trait and sipx ships no implementation of one. sipx is a
      stack, not a client of a particular vendor's push transport; the [vision](../vision.md)'s
      non-goals rule out anything else, and a test double is what the tests use.
- [x] `pn-purr` is handled or explicitly deferred with a reason recorded — it exists so a
      mid-dialog request can be matched to a stored binding, which only matters once something
      stores them.
- [x] The RFC registry entry for RFC 8599 moves off "not started", with the `Roles` column saying
      the UA half only.
- [x] Failing-first test: `a_push_wakes_a_client_that_refreshes_its_binding_before_the_invite`.

## Progress
- Implemented, UA half only, on `impl/T-21`. The syntax lives in `sipx-sip/src/push.rs` —
  `Device` for §8.7's `pn-*` URI parameters (validated against RFC 3261 §25.1's `pvalue`, since a
  bad token silently becomes a different URI), `Indicators` for §8.2's three feature-capability
  indicators read out of `Feature-Caps` (RFC 6809 §4, new `HeaderName::FeatureCaps`), and 555 as
  named constants. The behaviour lives in `sipx-ua`: `push::PushService` is the trait (no
  implementation shipped; the tests use a stub), `push::Support` is what a REGISTER response said
  (`supports()` answers §8.2's question), `Config::with_push` puts the parameters inside the
  `Contact` URI's angle brackets, `registrar::interpret` returns `Outcome::PushNotSupported` for
  555 and `UserAgent::register` surfaces it as `Error::PushNotSupported`, and
  `UserAgent::woken` is §4.1.3's ordering as a type — it sends the binding-refresh REGISTER and
  only then hands back a `push::Pending` that licenses expecting the pending request.
- `pn-purr` is read (`sipx_sip::push::purr`, `Support::purr`) and carried, deliberately not
  matched against stored bindings: matching is for the party that *stores* bindings, which is
  §5.6's proxy role and out of scope per the story's Notes. The reason is recorded on both `purr`
  doc comments.
- Registry entry for 8599 is `partial`, roles `["uac"]`; `docs/compliance.md` regenerated.
- All four tests in `crates/sipx-ua/tests/push.rs` pass, plus unit tests in both `push.rs` files.
- Review round 1 settled three things, none of them a change to what the story asked for.
  **555 keeps `sipx-cli register`'s exit code at 3**: modelling it as its own error had made it
  fall through to `Exit::Failed` = 1, and the published CLI reference documents 3 as "the far end
  refused", which a 555 is. **`interpret` returns `PushNotSupported` only when the registration
  named a push service** — to a client that sent no `pn-*` parameters the same code is a refusal
  this side has no reading of, so it reports the number. **`in_contact` verifies the parameters
  went in**: a `tel:`/`urn:`/`http:` contact parses and then has no `uri-parameter` list, so
  setting them was a silent no-op and the device would have been unreachable by the one mechanism
  it was configured for; the contact now comes back unchanged with a warning, which is what its
  doc comment had always claimed. A valueless `+sip.pns`/`+sip.pnspurr` no longer names the empty
  string. Unit tests added for each, plus direct ones for `Params::remove`/`Uri::remove_param`.
- `main` is merged in (not rebased) and `./scripts/gate.py` — which replaced the command list in
  `AGENTS.md` when X-22 landed — reports 12 steps all green, MSRV and docs site included.

## Notes
- The ordering in §4.1.3 is the whole mechanism and the easiest thing to get backwards. The push is
  not the call; it is permission to go and get a flow, and the INVITE arrives down the flow.
- The story is smaller than it looks precisely because sipx implements no push provider. What it
  owes is the SIP half: the parameters, the option negotiation, 555, and the refresh ordering.
- Scope: the client side. The proxy behaviour of §5.6 — holding the request in a push bucket while
  the client wakes — is a proxy role and belongs where the other proxy roles do, outside this repo.
