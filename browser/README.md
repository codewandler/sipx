# @sipx/browser — experimental

Browser ESM SIP phone with a Rust WASM signaling kernel and native WebRTC audio. This pre-1.0 API
is experimental (`experimental === true` at import); no registry publication is implied.

```js
import { SipxClient } from '@sipx/browser';
const client = await SipxClient.create({
  aor: 'sip:browser@example.net',
  auth: { username: 'short-lived-user', password: 'server-issued-secret' },
  transport: { scheme: 'wss', host: 'edge.example.net:443', resource: '/sip' },
  insecure: 'refuse',
});
client.on('outgoing', call => call.on('state', state => console.log(state)));
client.on('incoming', call => { /* show an explicit Answer action calling call.answer() */ });
await client.register({ expires: 600 });
const call = await client.dial('sip:target@example.net');
call.mute(true);
await call.hangup();
await client.close();
```

The `host` includes an optional port; there is no separate transport port field. Serve the package
modules and `sipx_browser.wasm` on the page's origin. The loader resolves the packaged asset relative
to its module, or accepts a same-origin `wasmURL`; cross-origin URLs and redirects are refused.
There are no runtime Node dependencies and no remote code downloads.

`dial()` and `answer()` resolve only after SIP establishment, validated negotiated profile facts
and the native peer connection are all ready. `outgoing` exposes the handle during setup, allowing
ringing UI and early hangup. `incoming` exposes an incoming offer without acquiring a microphone.
Microphone acquisition follows explicit dial/answer; use `setMicrophone(deviceId)` for selection.
Applications needing permission before server authorization can preflight capture and stop those
application-owned tracks before calling the SDK. Stream ownership is not transferred implicitly.

Register/dial/answer accept `AbortSignal`. Cancellation sends the inverse command and stops media
before settling. `close({timeoutMs:3000})` refuses new work, bounds SIP cleanup, frees the kernel,
closes the socket and media, then delivers `closed` before resolving. Page teardown synchronously
releases resources. State callbacks are ordered and deferred; one listener throwing cannot block
others. A fatal ABI defect suppresses further callbacks. Diagnostics never include SIP messages,
credentials, SDP or listener exception strings.

Errors have distinct exported classes: `SipxAbiDefect`, `SipxStateError`, `SipxLimitError`,
`SipxTransportError`, `SipxSipError`, `SipxMediaError`, `SipxCancelled`, and `SipxCapabilityError`.
`call.negotiatedMedia()` keeps validated kernel facts separate from browser observations;
`refreshStats()` refreshes optional native statistics. Missing observations remain absent.

The media adapter waits for complete ICE gathering and sends unchanged browser descriptions.
SIP, Digest, SDP validation and timers remain in Rust. ABI functions, commands, events and error
codes are generated from `crates/sipx-wasm/src/contract.rs`, consumed by the actual kernel, using
`scripts/generate-browser.py`; four independent mutation tests enforce drift detection.

Build and validate from the repository root:

```sh
./scripts/check-browser-binding.sh
python3 scripts/pack-browser.py --output scratch/package-candidate
cargo build -p sipx-call --example browser_sdk_proof --features dtls,opus
python3 tests/browser-sdk/prove.py \
  --archive scratch/package-candidate/sipx-browser-0.1.0.tgz \
  --output scratch/package-proof --chrome /path/to/chrome --driver /path/to/chromedriver
```

Packing rebuilds WASM, includes generated declarations, dual licenses, per-file checksums and
source provenance, and retains one explicit archive. Dirty candidates identify their base and
source digest. Final delivery is repacked from committed source and reruns the exact archive proof.
The clean consumer installs only that archive and compiles with TypeScript before served native
browser tests. The proof checks both SIP roles, challenges, audible sample energy and six independent
negative fixtures with zero remaining SDK resources. Fixture SDP mutations are isolated to tests.

Browser support is determined by executed proof evidence, not capability detection alone. The
current proof runner records negotiated Chrome/driver versions. Firefox and WebKit are explicitly
unsupported until their rows execute; no unexecuted version is claimed. Full repository acceptance
is the coordinator's `scripts/gate.py` run; package creation is not npm publication.
