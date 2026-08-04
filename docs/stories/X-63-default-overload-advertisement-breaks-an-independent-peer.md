---
id: X-63
title: Default overload advertisement breaks an independent peer
pillar: Signalling
status: done
priority: 3
areas: [sipx-transport, interop]
announcement: 3
---

# Default overload advertisement breaks an independent peer

## Goal

Keep RFC 7339/7415 overload control available without adding its capability parameters to every
ordinary endpoint by default, so a peer that does not implement the extension can still parse the
base SIP request.

## Acceptance

- [x] Overload capability advertisement is explicit endpoint configuration and is off by default.
- [x] An enabled endpoint retains the complete `loss,rate` offer and all existing admission and
      feedback behavior.
- [x] The independent-peer matrix passes all claimed signalling transports after the harness exit
      propagation from `X-62` is active.
- [x] The specification, public configuration docs and RFC registry say where overload support is
      enabled rather than claiming every endpoint advertises it.

## Progress

- Found after `X-62` stopped hiding failed role tests. One profile rejects the RFC-valid quoted
  `oc-algo="loss,rate"` list at its comma, on UDP as well as WebSocket, and never answers. RFC 7339
  §2 scopes its normative client behavior to an entity that supports the extension; §4.1/§4.2 then
  require `oc` and `oc-algo` on every request from that supporting client. The narrow compatible
  policy is therefore an explicit opt-in that retains the exact offer when selected—not rewriting
  or weakening the offer for one peer.
- `OverloadConfig::advertise` is now false by default. Enabled endpoint tests retain the complete
  quoted `loss,rate` offer, matching feedback, rate/loss admission and local rejection counters; a
  new default-endpoint test proves an ordinary request has no overload parameters or client state.
- With `X-62` preserving failed role exits, a fresh container run passed both independent profiles
  on UDP, TCP, TLS, WebSocket and secure WebSocket. The transport specification, RFC registry and
  generated public compliance table now state that only an explicitly configured supporting client
  advertises the extension; server feedback to an offered extension remains enabled.
