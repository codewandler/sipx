---
id: T-33
title: Bind browser WebSocket signalling
pillar: Transport
status: in-progress
priority: 24
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

- [x] The JavaScript binding opens browser WebSocket/WSS connections, selects the SIP subprotocol,
      feeds received bytes to the kernel and sends only bytes emitted by it.
- [x] WSS is the default and insecure WS requires an explicit development-only policy. Credentials
      are never placed in URLs, logs or thrown error strings.
- [x] Connect, send and receive queues have specified limits; overflow, close, browser offline and
      reconnect decisions surface as typed events and never start an unbounded retry loop.
- [x] Kernel timer requests use host timers but re-enter only as fired-timer inputs. Cancellation
      closes the socket and clears every owned timer and callback.
- [x] Tests cover fragmented delivery, coalesced messages, backpressure, close during authentication,
      stale callbacks after cancellation and non-root WebSocket paths.
- [ ] A browser fixture reaches a real WSS endpoint without adding I/O to `sipx-sip` or `sipx-sdp`;
      feature checks and the gate are green.

## Progress

### 2026-08-08 — the binding, in `browser/`

Delivered `browser/src/{transport,records,platform,index}.mjs` under
[`docs/specs/browser-signalling.md`](../specs/browser-signalling.md), a new spec written for this
story under `browser-sdk.md` as its parent. Browser-targeted ESM, no dependencies, no build step,
no `package.json` — `A-17` owns packaging, and the parent §7.1 requires the shipped package to have
no Node runtime dependency. Checked by `scripts/check-browser-binding.sh`, wired as a gate step and
a step of CI's existing `wasm` job (`./scripts/gate.py --check` → 47 steps over 22 CI jobs, clean).

**Failing-first.** All 42 cases written before any implementation and run red at `18a1c4d`:

```
$ node browser/test/run.mjs
browser signalling binding — 42 cases
  FAIL the URL is built from the transport block, WSS by default
         Error: signallingUrl is not implemented
  FAIL start() opens one socket and asks for the sip subprotocol
         Error: WebSocketSignalling is not implemented
  … 40 more …
42 case(s) failed
```

Four of them passed against the first stub for the wrong reason — `equal(error.reason,
Reason.Whatever)` holds when both sides are `undefined` — so a `refuses()` helper was added that
also asserts the reason is a non-empty string, and the run above is after that fix. Green now at
48 cases: 44 against injected fakes, 4 driving the compiled `sipx_browser_wasm.wasm`.

**Queue limits, and where each number comes from.** Send and receive queues are both 256 records
or 256 KiB — the kernel's own `MAX_QUEUED_RECORDS`/`MAX_QUEUED_BYTES` from §4.9, because that pair
is the kernel's statement of how much in-flight signalling is a defect rather than a burst, and it
is exactly tight for the send side (one drain can hand the binding no more than the kernel queued).
A case reads `crates/sipx-wasm/src/bounds.rs` and holds the JavaScript to it, so they cannot drift.
`bufferedAmount` past 256 KiB is backpressure and fails closed rather than growing the platform's
buffer. Entropy refill is 960 = capacity 1024 − low-water 64, the largest feed that can never
overflow the pool.

**Bounded reconnection.** 7 attempts for the **lifetime of one binding**, delays doubling from T1
(500…16 000 ms, summing to 31 500 = 63 × T1, inside RFC 3261's 64 × T1 horizon). The budget never
replenishes — replenishing on a successful open makes a flapping peer an unbounded retry loop,
which is the thing this row forbids; retry past the budget is the application's under parent §5.3.
Offline suspends without spending or refunding attempts. Subprotocol refusal and framing violations
are not retried at all.

**Stale callbacks.** Every socket and timer callback is guarded on socket identity and on the
closed state, counted in `counters.staleCallbacks` and dropped. `FakeClock.fireEveryRetainedCallback`
invokes every callback ever scheduled, cleared or not, after cancellation — the kernel is not
entered and no listener runs.

**Framing.** `docs/specs/sip-tls.md` §4 says a fragment and a coalesced frame both close the
connection. The binding offers each frame whole and once, holds no cross-message state, and reads
the kernel's `parse_errors` to learn the verdict — it will not parse SIP in JavaScript, which
parent §8.1 concentrates in the kernel on purpose. Measuring that against the real module found
that the kernel refuses a fragment but **silently truncates a coalesced frame**; filed as `S-53`
with the measurements, and `docs/specs/browser-signalling.md` §5 states the limit rather than
implying the binding closes it.

**No Rust changed.** `git diff --stat 18a1c4d -- crates/ wasm/src` is empty, so `sipx-sip` and
`sipx-sdp` gained no I/O by construction rather than by inspection.

**The unticked row.** A real browser reaching a real WSS endpoint needs a matched browser, a
WebDriver and a TLS listener — the constraint `browser-audio` already lives under as a CI-only job.
Split out as `T-43` rather than faked. Everything else in that row is proven: no Rust changed, and
`./scripts/check-features.sh` was not touched because no feature graph moved.

**Not run:** the full gate (`./scripts/gate.py`), by dispatch. Verified instead with
`./scripts/check-browser-binding.sh`, `./scripts/check-wasm-kernel.sh`, `./scripts/gate.py --check`,
`python3 scripts/test-gate.py`, `./scripts/check-fixed-sleep.py --check`,
`./scripts/check-provenance.sh` and `cargo fmt --all --check`.
