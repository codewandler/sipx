---
id: T-23
title: Let a WebSocket target name its own path and port
pillar: Signalling
status: done
priority:
design: docs/designs/sip-transport.md
epic: depth
areas: [sipx-transport]
note: found by X-17 — the second interop peer serves SIP over WebSocket somewhere sipx cannot ask for
---

# Let a WebSocket target name its own path and port

## Goal
Reach a WebSocket peer that serves SIP anywhere other than `/` on the SIP port, because the
second interop peer does and sipx has no way to say so.

## Acceptance
- [x] A `Target` for `TransportKind::Ws`/`Wss` carries the request path, defaulting to `/` so
      nothing that works today changes.
- [x] The path reaches the handshake: `crates/sipx-transport/src/ws.rs:91` builds the request URI
      as `{scheme}://{authority}/` with the path hardcoded.
- [x] Failing-first test: a fixture WebSocket server that answers only on `/ws` and 404s on `/`,
      which sipx currently cannot register through.
- [x] `tests/interop/asterisk/profile.sh` drops `registers_against_a_real_server_over_websocket`
      from its `PEER_DIVERGES_ON` list and the test passes against the second peer, on the port
      that peer's HTTP server binds.

## Progress
- Filed by `X-17` as the one thing the second interop peer and sipx disagree about.
- Done, and verified live against both interop peers. `Target::at_path` carries the resource
  (leading `/` supplied when missing, query strings kept); it rides on `ConnectionKey` so two
  resources on one address are two connections, for the same reason two verified names are.
  `ws.rs`'s upgrade request takes the path instead of hardcoding `/`. Fixture: `tests/ws.rs`'s
  `a_peer_serving_sip_only_at("/ws")`, which 404s the root — reached when the target names the
  resource, refused when it does not (W13 in `docs/specs/sip-tls.md`). The shared interop test
  reads `SIPX_INTEROP_WS_PORT`/`SIPX_INTEROP_WS_PATH` from the peer profile, defaulting to the
  SIP port and `/`; the second peer declares `8088` and `/ws` and its `PEER_DIVERGES_ON` is now
  empty. `./tests/interop/run.sh` passed against both peers with no skips.

## Notes
- RFC 7118 §5 describes the WebSocket subprotocol `sip` and the handshake, and says nothing
  about the resource name — so both readings are legal and this is a gap in sipx's expressive
  range, not a defect in either implementation. The first peer accepts the upgrade on any path,
  which is why one peer could not have found this.
- Reproducible against the second peer as configured today:

  ```text
  GET /ws  on 127.0.0.1:8088 → upgraded, subprotocol sip
  GET /    on 127.0.0.1:8088 → HTTP/1.1 404 Not Found (Server: Asterisk/20.20.1)
  ```

- There are two halves and the second is the smaller one: the peer serves WebSocket from its own
  HTTP server on its own port, so a `Target` that takes the path still needs the caller to point
  it at a different port from the SIP one. `Target::new` already takes a `SocketAddr`, so that
  half is a harness concern rather than a transport one.
