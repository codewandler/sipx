# The browser signalling binding

The browser half of the browser SDK: the code that opens the WebSocket and drives the sans-I/O
session kernel over it. The kernel is Rust compiled to WebAssembly and lives in
[`../crates/sipx-wasm`](../crates/sipx-wasm) and [`../wasm`](../wasm); nothing in this directory
parses SIP, holds protocol state or reaches the network except through the four platform
facilities named below.

- **Contract:** [`../docs/specs/browser-signalling.md`](../docs/specs/browser-signalling.md), under
  its parent [`../docs/specs/browser-sdk.md`](../docs/specs/browser-sdk.md).
- **Story:** `T-33`.
- **Checked by:** `../scripts/check-browser-binding.sh`, which is a gate step and a step of CI's
  `wasm` job.

## Layout

| Path | What it is |
|---|---|
| `src/transport.mjs` | `WebSocketSignalling`, the binding, plus `signallingUrl` and the bounds |
| `src/records.mjs` | the parent §4.6 output-record framing, decoded |
| `src/platform.mjs` | the only file that reads a global: the real socket, clock, CSPRNG and connectivity monitor |
| `src/index.mjs` | the public surface |
| `test/` | the cases, the fakes, and a hand-written ABI port used to drive the compiled module |

Browser-targeted ESM throughout, with no dependencies and no build step. `A-17` packages this as
part of `@sipx/browser` and generates the ABI glue that today's `test/kernel.test.mjs` writes by
hand; `M-52` adds the media adapter; the `SipxClient` lifecycle layer is `A-17`'s.

## Running the cases

```sh
node browser/test/run.mjs                       # against fakes only
node browser/test/run.mjs --module <path.wasm>  # and against the compiled kernel
./scripts/check-browser-binding.sh              # builds the module, then does both
```

There is no test runner and no `package.json` here on purpose: the repository has no JavaScript
toolchain, `wasm/harness.mjs` established that a self-asserting script run by a checker is enough,
and the parent §7.1 requires the shipped package to have no Node runtime dependency. `test/` uses
Node only to read files; `src/` uses nothing but the web platform.

## The one thing to know before changing it

The binding takes its socket, clock, entropy source and connectivity monitor as constructor
arguments. That is not dependency-injection decoration — it is what lets every case decide a
race at an exact instant, and it is why no case in this directory waits on wall-clock time. A
change that reaches for a global from `src/transport.mjs` has removed the property the suite is
built on. `src/platform.mjs` is where globals live.
