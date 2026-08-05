---
id: T-33
title: Bind browser WebSocket signalling
pillar: Transport
status: backlog
priority: 15
design: docs/designs/browser-sdk.md
epic: browser-sdk
areas: [browser, websocket, wss, wasm, m15]
predicate:
announcement:
note: after A-16 and S-41 · browser owns I/O, WASM core consumes bytes
---

# Bind browser WebSocket signalling

## Goal

Drive the WASM session kernel over the browser's WebSocket API with bounded queues, explicit
readiness and fail-closed secure defaults.

## Acceptance

- [ ] The JavaScript binding opens browser WebSocket/WSS connections, selects the SIP subprotocol,
      feeds received bytes to the kernel and sends only bytes emitted by it.
- [ ] WSS is the default and insecure WS requires an explicit development-only policy. Credentials
      are never placed in URLs, logs or thrown error strings.
- [ ] Connect, send and receive queues have specified limits; overflow, close, browser offline and
      reconnect decisions surface as typed events and never start an unbounded retry loop.
- [ ] Kernel timer requests use host timers but re-enter only as fired-timer inputs. Cancellation
      closes the socket and clears every owned timer and callback.
- [ ] Tests cover fragmented delivery, coalesced messages, backpressure, close during authentication,
      stale callbacks after cancellation and non-root WebSocket paths.
- [ ] A browser fixture reaches a real WSS endpoint without adding I/O to `sipx-sip` or `sipx-sdp`;
      feature checks and the gate are green.

## Progress

- Backlog. Depends on A-16 and S-41.
