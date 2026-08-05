---
id: A-20
title: A WSS client for non-SIP peers
pillar: Application
status: done
priority: 2
design: docs/designs/openai.md
epic: openai
areas: [sipx-app, tls, ws]
predicate:
announcement:
note: independent of A-19 — a general RFC 6455 client over the existing TLS policy, no vendor knowledge
---

# A WSS client for non-SIP peers

## Goal

Give `sipx-app` a general-purpose secure WebSocket client — RFC 6455 over the workspace's
one TLS policy — so an application component can speak to a non-SIP peer at all;
`sipx-transport`'s client refuses any peer that does not negotiate the `sip` subprotocol,
by contract, and must stay that way.

## Acceptance

- [x] `sipx-app` exposes a client that connects to a `wss` URL by composing
      `tokio-tungstenite`'s handshake over `sipx-transport`'s `ClientTls` — the same trust
      anchors, verification and refusal behaviour as every other TLS client in the
      workspace, proven by a failing-first test that a wrong-name certificate and an
      unknown-issuer certificate are refused with the existing typed errors.
- [x] The caller supplies request headers (at least `Authorization`); no subprotocol is
      required or offered unless the caller names one.
- [x] Frame and message sizes are bounded via the handshake configuration; an oversize
      message is a typed error, not an allocation.
- [x] Liveness follows the session-binding discipline: Ping answered, a peer silent past the
      bound surfaces as a typed close — and the bound is a failure bound, not a
      happens-before (`check-fixed-sleep.py` clean).
- [x] Cleartext `ws` to non-loopback hosts is refused; loopback is permitted so A-21's
      stand-in peer and this story's own tests can run without certificates where the spec
      allows it.
- [x] No new workspace dependency, and `tokio-tungstenite` keeps its no-TLS-features stance
      (one TLS policy, not two) — asserted the way the Cargo.toml comment states it.
- [x] `scripts/check-app-surface.py` output reviewed: whatever this adds to the supported
      surface is deliberate and named in the story's Progress.

## Progress

- 2026-08-05 (`impl/A-20`): implemented as `crates/sipx-app/src/wss.rs` — `WssClient` /
  `WssRequest` / `WssConnection` / `WssMessage` / `WssError`, `tokio-tungstenite`'s client
  handshake composed over `sipx_transport::tls::ClientTls` on a `TcpStream`. TLS refusals pass
  through as the existing `TlsError::Handshake`; cleartext `ws` is refused for any non-loopback
  *name* (resolution is never consulted); frame and message bounds go into the handshake
  configuration; liveness is Ping on a cadence with a grace, a silent peer surfacing as the
  typed `WssError::Stalled`, and the peer's Pings answered by the protocol layer. Proven by
  `crates/sipx-app/tests/wss_client.rs` (13 tests, failing-first on the wrong-name vector) plus
  10 unit vectors in the module; fixture certificates come from
  `cargo run -p sipx-testkit --example issue-certs` into the system temp dir per run.
- 2026-08-05 (`impl/A-20`, review rework): a URL's RFC 3986 §3.2.1 userinfo was reaching the
  `Host` header, every error's `peer`, and `WssRequest`'s `Debug` — `Authority::as_str()` keeps
  it. Now refused (not stripped: stripping would dial on unauthenticated and leave the peer's
  401 to explain why), the authority that travels is rebuilt from the parsed host and port, and
  every printed URL goes through `redacted`, which reads the raw string so a URL too malformed
  to parse still cannot leak. Also: `WssError::Subprotocol` was unreachable — the dependency's
  handshake refuses first — so it is now that refusal re-typed and the test asserts the variant;
  the buffered-Pong drain closes a false-`Stalled` window a caller-driven `next` has and a
  server's parked loop does not; and the strict-Pong consequence (a data-streaming, Pong-dropping
  peer is `Stalled` mid-burst) is stated in the module docs so `A-22` inherits it knowingly.
- 2026-08-05 (`impl/A-20`, review rework 2): the redaction only covered URLs containing `://`,
  and `user:sekret@host` / `//user:sekret@host` both parse as a `Uri` without a scheme — so a
  pasted URL that lost its `wss://` reached the scheme refusal with the credential intact. The
  scheme is now optional in `redacted`. Second, the previous round's drain had made the
  strict-Pong rule unreachable: any ready data frame short-circuited the search, so a peer
  streaming while withholding Pongs deferred the verdict frame by frame. The drain now takes
  the whole buffered backlog looking for a Pong (data set aside in `held`, delivered in order,
  never dropped) and the `select!` is `biased` with the clock first, so an expired deadline is
  settled before another frame is handed over. Both liveness properties now hold at once and
  each has its own vector; module docs, code comments and this note say the same thing.
- `scripts/check-app-surface.py` reviewed: before, "10 of 11 published crates; 7 modules and 0
  crates experimental"; after, the same closure with **8** modules experimental — the new `wss`
  module itself, marked **Experimental** (`A-8`) deliberately. No crate joins the dependency
  closure and nothing graduates, so the *supported* surface is unchanged; the module graduates
  when `A-22`'s bridge (or an external caller) constrains its shape.

- 2026-08-05 (integration): independent review PASS after two rework rounds; merged and gated
  green (36 steps). Five minors accepted rather than reworked, three of which are contracts
  `A-22` inherits and one of which is a sentence that is wrong rather than code that is:
  **`Stalled` is not sticky** — after it returns, `held`/`awaiting_pong`/`probe` persist and a
  caller that calls `next` again is handed each held message before `Stalled` recurs, so a
  consumer must treat the first one as terminal; **the fatal drain's backlog is discarded** on
  that path, which contradicts the field doc's "owed to the caller … never dropped" (the
  behaviour is right for a connection just declared dead, the sentence is not); **`held` has no
  aggregate cap**, bounded only by what one liveness pass finds already buffered, which is a
  local or fast hostile peer's opening and only while a grace is expired; and
  `WssError::Subprotocol` with an empty `offered` renders with a double space and reads
  backwards for the peer-named-one-nobody-offered case. Filed forward into `A-22`'s Notes
  rather than fixed here, because the bridge is the caller that will constrain them.

## Notes

- Design: `docs/designs/openai.md` component 2. See `crates/sipx-transport/src/ws.rs` for
  the SIP-specific client this deliberately does not generalize, and
  `crates/sipx-transport/src/tls.rs` for `ClientTls`.
- The tests need a loopback TLS WebSocket peer; `cargo run -p sipx-testkit --example
  issue-certs` generates fixture certificates per run — never commit certificate material.
