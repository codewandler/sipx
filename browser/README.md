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

## Native media adapter

`BrowserMediaAdapter` in `src/media.mjs` owns browser audio resources, and
`browserMediaPlatform` in `src/media-platform.mjs` supplies its browser globals. The adapter
accepts `{ platform, sendCommand, onState, onError, onDiagnostic, setupTimeoutMs,
operationTimeoutMs }`; defaults bound setup to 30 seconds and each browser operation to 10 seconds.
Hooks are internal lifecycle inputs, not application callbacks; the packaged client owns public
callback scheduling and command outcome promises. The sender accepts `{cmd, call, ...}` and the
client supplies command version and correlation id. The sender must synchronously enqueue or
throw; return values are ignored and asynchronous senders are unsupported. Kernel command
outcome promises belong to the lifecycle.

Deliver every kernel event through `handle(event)` without awaiting pending media work before
forwarding cancellation. `need-local-media` follows the user's dial/answer gesture; construction
and incoming offers acquire no microphone. A remote offer is applied without a kernel command;
a remote answer emits `media-applied` only after application succeeds. Failure while applying an
answer uses `media-failed`; every other media failure requests `hangup` and emits the separate
`SipxMediaError` kind. The client must preserve that media cause while completing SIP cleanup.

The adapter submits unchanged, complete-gathered descriptions and consumes the kernel's validated
`negotiated-media` event; it never parses SDP. `onState({call,state:"established"})` requires both
kernel establishment/profile facts and browser connectivity. `refreshStats(call)` updates a
`negotiatedMedia(call)` report with separate `kernel` and `browser` origins; missing fields remain
absent. `setMicrophone(deviceId)` selects the device for subsequent capture and `mute(call, true)`
changes only local track enablement.

`closeCall(call)` and `close()` synchronously revoke media ownership. Terminal kernel events and
pagehide use the same cleanup; late permission results stop their tracks. Platform playback
returns `{ready, close}` synchronously, so a pending autoplay promise cannot retain a resource.
The injected platform also supplies `getUserMedia`, `createPeerConnection`, `codecs`, `clock`
(`setTimer`, `clearTimer`) and `onPageHide` (returning an unsubscribe function).

Run `node --test browser/test/media.test.mjs` for deterministic media cases. These do not replace
the dependent package story's real browser proof.
