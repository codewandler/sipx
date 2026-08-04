# Design: browser audio SDK

**Status:** proposed · **Pillar:** Application · **Epic:** `browser-sdk` · **Stories:** A-16, S-41,
T-33, M-52, A-17, A-18, X-100

## Why

Beta.4 proves that sipx can interoperate with a browser audio endpoint, but it does not let a web
application use sipx itself. The requested product surface is an installable JavaScript and
TypeScript SDK generated around a WebAssembly SIP/session kernel, a browser WebSocket binding, a
browser-native WebRTC audio adapter, and a runnable demonstration site.

This remains compatible with the vision's WebRTC boundary: sipx does not implement capture, render,
ICE sockets, DTLS, SRTP or a browser media engine in WebAssembly. The browser's `RTCPeerConnection`
owns those. The compiled Rust is the sans-I/O SIP, SDP, transaction and dialog logic; JavaScript
delivers bytes, timers and browser media descriptions at explicit interfaces.

## Approach

`A-16` writes the normative ABI, lifecycle, security, package and browser-support contract before
code and distinguishes this SDK from the server-side application contract in `A-3`. `S-41` makes the
selected sans-I/O kernel compile to WebAssembly with host-supplied entropy and timers. `T-33` binds
the browser's WebSocket/WSS API without putting I/O in the core. `M-52` translates the delivered
audio-only profile to `RTCPeerConnection` and refuses video, data channels and silent security
downgrades.

`A-17` generates checked JavaScript and TypeScript surfaces from the ABI and packages them for a
normal browser toolchain. `A-18` publishes a static demonstration website that registers, dials,
answers, hangs up, reports negotiated security and exchanges non-silent audio. `X-100` runs the
packaged SDK and demo in the supported browser matrix with bounded fake-media fixtures and negative
tests.

## Exit

A clean JavaScript consumer installs the package, serves the demo, registers over WSS and completes
audio calls in both SIP roles using browser-native WebRTC. The generated types match the shipped WASM
ABI, cancellation releases every timer/socket/media track, wrong fingerprints and weaker media fail
closed, and the public boundary still says audio-only and relay-limited until TURN lands.
