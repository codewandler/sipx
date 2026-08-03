---
id: T-29
title: Bind only the selected cleartext transports
pillar: Signalling
status: ready
priority: 2
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport]
predicate:
note: requested by sipx-clstr FC-1 — Config can express UDP or UDP+TCP, never TCP without an undeclared UDP socket
---

# Bind only the selected cleartext transports

## Goal

Let an endpoint select UDP only, TCP only, or both cleartext transports exactly, so binding TCP
does not silently widen its network exposure to UDP.

## Acceptance

- [ ] `Config` represents UDP-only, TCP-only and UDP+TCP explicitly. Each selection binds exactly
      those listener kinds; an empty selection is a typed pre-bind configuration error unless the
      endpoint has another configured signalling listener.
- [ ] UDP+TCP preserves today's same-address, same-port behavior, including the bounded retry when
      port `0` chooses a UDP port whose TCP counterpart is occupied.
- [ ] TCP-only with port `0` reports the TCP listener's chosen address through the public handle and
      uses that port for `Via` sent-by when no explicit advertised port was supplied. It does not
      create a placeholder UDP socket internally.
- [ ] Sending and receiving continue to work for every selected transport. Code that needs a UDP
      socket handles its absence explicitly rather than routing TCP-only traffic through a dummy
      datagram path.
- [ ] Failing-first test: `tcp_only_binds_no_udp_socket` requests TCP without UDP, connects to the
      reported TCP address, and proves a UDP socket can simultaneously bind that same address. The
      minimal configuration step does not compile against `v0.10.0`: `Config` has a mandatory
      cleartext `bind` plus `tcp: bool`, and `bind_matching_ports` always binds UDP before it can bind
      TCP. Companion tests pin UDP-only and the existing shared-port UDP+TCP case.
- [ ] The transport spec's configuration table and bind-state table are updated before the driver.

## Progress

- Filed from a downstream exact-exposure review. No implementation has started.

## Notes

- At `v0.10.0`, `sipx-transport/src/endpoint.rs::bind_matching_ports` begins every attempt with
  `UdpSocket::bind(config.bind)` and returns TCP only as an optional second listener. `tcp: false`
  means UDP-only and `tcp: true` means UDP+TCP; no public value means TCP-only.
- Requested by downstream
  [sipx-clstr FC-1](https://github.com/codewandler/sipx-clstr/blob/main/docs/stories/FC-1-refuse-a-transport-the-node-cannot-serve.md)
  through its [upstream ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md).
  That platform must refuse TCP-only configuration until a released sipx version carries this
  capability; it must not copy the endpoint driver.

