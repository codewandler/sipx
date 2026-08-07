---
title: Use sipx as a library
description: The crates are useful separately — a parser and transaction machines with no async runtime, offer/answer as a pure function, or the whole call framework.
---

# Use sipx as a library

The crates are useful separately. Take the pure protocol core, the async transport and user-agent
layers, or the complete call framework. The current public prerelease is on crates.io; `main` can move
ahead of it.

## Stability policy

Public APIs are not frozen before 1.0. Each crate-level API page labels its surface Supported or
Experimental. Breaking Supported APIs receive a changelog entry and migration guidance;
Experimental APIs may change shape or be removed without a migration note. The labels describe
support intent, not semantic-version stability.

## Add a dependency

Pin every directly imported sipx crate to the exact public prerelease. This complete minimal block
compiles the [answer-a-call example](answer-a-call.md):

<!-- BEGIN generated:answer-consumer-dependencies -->
```toml
[dependencies]
sipx-call = "=1.0.0-rc.2"
sipx-sip = "=1.0.0-rc.2"
sipx-transport = "=1.0.0-rc.2"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```
<!-- END generated:answer-consumer-dependencies -->

This website documents `main`, which can be tested explicitly with:

```toml
[dependencies]
sipx-call = { git = "https://github.com/codewandler/sipx", branch = "main" }
```

Commit `Cargo.lock` for applications and binaries so the selected Git revision is reproducible.
For a library, use the crates.io dependency unless you intentionally require unreleased work.

Optional behavior is opt-in. For example, make Opus selectable by `sipx-call` like this:

```toml
[dependencies]
sipx-call = { version = "=1.0.0-rc.2", features = ["opus"] }
```

Opus links a C library. Enabling the feature makes `Codecs::Opus` available; it does not silently
change the codecs a call offers. G.711 remains the default selection.

The named browser-audio call policy needs both optional media boundaries and still has to be
selected explicitly:

```toml
[dependencies]
sipx-call = { version = "=1.0.0-rc.2", features = ["opus", "dtls"] }
```

### Opus packaging policy

Opus is off by default in `sipx-audio`, `sipx-media`, `sipx-call`, and `sipx-cli`; no shipped
application enables it implicitly. Selecting it brings in the native `libopus` boundary through the
optional `opus` and `audiopus_sys` Rust packages, so a deployment needs the native library available
or must deliberately accept building it from source.

The Rust packages carry MIT OR Apache-2.0 licensing and remain inside the workspace's permissive
Cargo licence policy. Native-library distribution is a separate packaging boundary and should be
reviewed for the target platform. The unmaintained `audiopus_sys` advisory RUSTSEC-2026-0150 is the
one narrow exception in `deny.toml`: CI uses a system `libopus` through `pkg-config`, avoiding the
advisory's source-build failure, and the exception ends when a maintained encoding-capable binding
is available or the opt-in codec no longer justifies it. This is why enabling Opus is an explicit
deployment decision rather than a default.

The current implementation uses the mandatory `opus/48000/2` RTP mapping with mono audio. It does
not advertise or apply optional RFC 7587 `fmtp` controls such as bitrate limits, in-band FEC, DTX,
CBR, or stereo preferences.

## Parse without a runtime

`sipx-sip` and `sipx-sdp` depend on **no async runtime at all**. Use them for a parser,
transaction state machines, or offer/answer as pure logic without taking a socket layer.

The example below is a real file that CI compiles
([`crates/sipx-sip/examples/parse_a_message.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-sip/examples/parse_a_message.rs)):

<!-- BEGIN generated:example crates/sipx-sip/examples/parse_a_message.rs -->
```rust
//! Parse a SIP message and read its headers, with no runtime and no sockets.
//!
//! `sipx-sip` is usable entirely on its own — this example depends on no async runtime at all.
//!
//! ```text
//! cargo run --example parse_a_message
//! ```

// These samples are read by people before they are run by machines, so they are written for
// readability where the workspace lints would prefer something terser. `clone_into` over
// `to_owned` teaches nothing in a five-line example, and a sine wave has to become an `i16`
// somewhere.
#![allow(clippy::assigning_clones, clippy::cast_possible_truncation)]

use bytes::Bytes;
use sipx_sip::{HeaderName, Limits, Message, parse_datagram};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let wire = b"INVITE sip:bob@example.com SIP/2.0\r\n\
        Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bK776asdhds\r\n\
        Max-Forwards: 70\r\n\
        To: Bob <sip:bob@example.com>\r\n\
        From: Alice <sip:alice@example.net>;tag=1928301774\r\n\
        Call-ID: a84b4c76e66710@example.net\r\n\
        CSeq: 314159 INVITE\r\n\
        X-Vendor-Thing: preserved verbatim\r\n\
        Content-Length: 0\r\n\r\n";

    let message = parse_datagram(Bytes::from_static(wire), &Limits::datagram())?;

    let Message::Request(request) = message else {
        return Err("expected a request".into());
    };
    println!("{:?} {}", request.method, request.uri);

    // Typed access is lazy: the header is parsed when it is asked for, not when the message is.
    let via = request.headers.typed::<sipx_sip::headers::Via>();
    if let Some(Ok(via)) = via {
        println!("came from {}", via.host);
    }

    // A header sipx has no behaviour for still survives intact. That is why "parse-only" is a
    // status in the compliance table rather than a gap in it.
    if let Some(value) = request
        .headers
        .value(&HeaderName::Other("X-Vendor-Thing".into()))
    {
        println!("unknown header kept: {}", String::from_utf8_lossy(&value));
    }

    // And it re-serializes byte for byte.
    assert_eq!(
        sipx_sip::Message::Request(request).to_bytes().as_ref(),
        wire.as_slice()
    );
    println!("round-tripped byte for byte");
    Ok(())
}
```
<!-- END generated:example -->

The message borrows its input bytes, preserves headers it does not interpret, and returns typed
parse errors. Transaction time enters as a fired-timer input and leaves as a set-timer output.
Those properties make the core deterministic to drive from another runtime or a unit test.

## Use explicit linear PCM

Application audio carries its own depth and rate. Playback converts that format to the negotiated
codec clock; capture converts received audio to the rate the caller selected. Nothing infers a
format from a byte count.

```rust
use sipx_audio::{Pcm, PcmEncoding, PcmFormat, PcmSamples};

let prompt = Pcm::new(
    PcmFormat::new(16_000, PcmEncoding::Unsigned8)?,
    PcmSamples::Unsigned8(raw_prompt),
)?;
call.play_pcm(&prompt).await?;

let wanted = PcmFormat::new(24_000, PcmEncoding::Signed16)?;
let mut capture = call.media().capture(wanted)?;
if let Some(chunk) = capture.recv().await {
    send_to_audio_consumer(chunk);
}
```

Supported application rates are 1 through 384,000 Hz. Unsigned 8-bit uses 128 as silence; signed
16-bit uses native `i16` samples. Unsupported rates and a depth/buffer mismatch are typed
`PcmError`s before audio is queued. The `sipx` command uses this same path for WAV input, so a WAV
whose header rate differs from the call is resampled rather than played at the wrong speed.

## Which crate

| You want | Take |
|---|---|
| Parse and build SIP messages, transactions, dialogs | `sipx-sip` |
| SDP and offer/answer | `sipx-sdp` |
| Sockets, TLS, WebSocket, RFC 3263 resolution, identity rotation and bounded endpoint policy | `sipx-transport` |
| Registration, digest authentication, event subscriptions and publication | `sipx-ua` |
| RTP, RTCP, jitter buffer, quality statistics, SRTP | `sipx-rtp` |
| G.711 (µ-law and A-law), G.722, L16, linear PCM conversion and resampling, WAV, and Opus behind the `opus` feature | `sipx-audio` |
| RTP/RTCP sockets bound to negotiated SDP with NAT handling, bridging, conferencing | `sipx-media` |
| Calls with playback, recording, DTMF, transfer, event services, application-owned dialog requests, and confirmed-dialog snapshots | `sipx-call` |
| Socket-free call signalling tests with seeded faults and virtual time | `sipx-testkit` |
| A phone to run rather than embed — the `sipx` binary | `sipx-cli` |
| The `sipx.app.v1` contract: its types, wire format and interpreter | `sipx-app-protocol` |
| The application host, webhook/session/realtime bindings, and deterministic contract harness | `sipx-app` |

`sipx-app` includes a `sipx-host` process that serves real calls to document-mode webhooks,
authenticated full-duplex sessions, or a configured realtime audio bridge. A granted session can
originate calls. The Rust host surfaces are Supported under the policy above; the `sipx.app.v1`
wire line remains Experimental, and no embedded runtime or TypeScript SDK is shipped.

## Operate a live transport endpoint

The transport handle exposes three deliberately narrow host seams. `Handle::observe(capacity)`
replaces the optional bounded receiver for parsed inbound and finalized outbound messages plus
connection lifecycle transitions. Producers never wait for it: overflow increments
`Counters::observation_dropped`, and dropping the receiver detaches observation.

`Config::request_policy` accepts a `RequestPolicyRef` that sees an immutable finalized request and
target before transaction creation. It may allow, reject, or add application-owned headers; route,
dialog, authentication and framing fields remain protected. `Handle::replace_source_admission`
atomically publishes a complete bounded `SourcePrefix` generation for new UDP sources and new
stream connections, before parsing or handshake work. Existing admitted connections retain the
generation that accepted them; `clear_source_admission` returns to allow-all.

With the `tls` feature and a configured TLS or secure-WebSocket listener,
`Handle::reload_server_identity` validates a complete `tls::Identity` and atomically selects it for
later handshakes. A malformed chain or mismatched key leaves the prior identity active. Existing
connections are neither renegotiated nor closed, and file watching or secret-store I/O remains the
embedding host's responsibility.

## Serve inbound event subscriptions

`sipx-call::Notifier` attaches to the same dispatcher that routes calls. Its handle observes the
exact `sipx_ua::subscribe::Subscriptions` allocation used by the socket path and exposes task and
shedding counters:

```rust
use std::time::Duration;
use sipx_call::{Dispatcher, Notifier};

let notifier = Notifier::new(Duration::from_secs(300), 128);
let observations = notifier.handle();
let dispatcher = Dispatcher::new(endpoint, incoming).with_notifier(notifier);

assert_eq!(observations.subscriptions().lock()?.capacity(), 128);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Polling `Dispatcher::next` serves `dialog`, `reg`, and `presence` SUBSCRIBE requests, sends the
required initial NOTIFY, and re-arms or terminates the one owned expiry task on refresh or
unsubscribe. This notifier API is Experimental. It sends valid empty full snapshots initially;
automatic projection of live calls, registrations and published presence into later documents is
not part of this surface yet.

## Place outbound event subscriptions

The matching Experimental subscriber is split at the same I/O boundary as the rest of sipx.
`sipx_ua::event_client::EventClient` is the deterministic state machine; it receives responses,
NOTIFY requests and fired timer generations as values. `sipx_call::EventSubscriptions` applies
those outputs through the dispatcher and a real endpoint:

```rust
use sipx_call::{Dispatcher, EventSubscriptions};
use sipx_ua::event_client::{Config as EventConfig, PackageConsumer, Start};
# async fn run<C: PackageConsumer>(endpoint: sipx_transport::Handle,
#     incoming: tokio::sync::mpsc::Receiver<sipx_transport::Incoming>,
#     start: Start<C>) -> Result<(), Box<dyn std::error::Error>> {
let endpoint_shutdown = endpoint.clone();
let events = EventSubscriptions::new(EventConfig::default())?;
let event_handle = events.handle();
let mut dispatcher = Dispatcher::new(endpoint, incoming).with_event_subscriptions(events);
let dispatch = tokio::spawn(async move { while dispatcher.next().await.is_some() {} });
let mut subscription = event_handle.subscribe(start)?;
if let Some(notification) = subscription.recv().await {
    let _package_value = notification.value;
}
subscription.unsubscribe().await?;
drop(subscription);
endpoint_shutdown.shutdown().await;
dispatch.await?;
# Ok::<(), Box<dyn std::error::Error>>(())
# }
```

`Start`'s consumer implements `PackageConsumer`; it declares the Event token, accepted media types,
neutral value and bounded synchronous body parser. The initializer also carries the selected target,
local Contact, fresh Call-ID/tag/CSeq, optional digest credentials and NOTIFY trust policy.

The full initializer is intentionally explicit: matching Call-ID/tags is correlation, not
authorization. The default `SamePeer` policy accepts NOTIFY only from the exact selected peer and
transport; proxy deployments inject a finite allow-list or authenticated policy. Every usage has a
provisional expiry before the first response, Timer N, one refresh timer, one in-flight SUBSCRIBE,
  a bounded delivery queue, non-wrapping CSeq and observable task/timer/transaction counts.

For registration discovery, the built-in `RegistrationConsumer` is the concrete package policy;
the lifecycle remains the same generic code:

```rust
use sipx_ua::reginfo::RegistrationConsumer;

let consumer = RegistrationConsumer::new("sip:alice@example.com", 4096)?;
// Put `consumer` in event_client::Start, then pass Start to event_handle.subscribe(...).
// Each accepted value is a complete RegistrationSnapshot, not a fragment.
# Ok::<(), Box<dyn std::error::Error>>(())
```

The consumer requires a version-zero full document before any partial document, applies exact-next
versions atomically, and retains at most the configured number of active contacts. Gaps, duplicate
contact identities, malformed XML, DTD/entity input and overflow are rejected without replacing the
last snapshot. `EventNotification::received_at` is the monotonic observation time; applications can
use `EventSubscription::next_event` to wait for either the first snapshot or a typed refusal.

## Publish event state through an endpoint

`sipx-call::Publications` is the matching Experimental RFC 3903 service in both roles. Attach it to
the dispatcher to route live inbound PUBLISH requests through the exact compositor allocation and
authorization policy supplied by the application:

```rust
use std::sync::Arc;
use std::time::Duration;
use sipx_call::{
    AllowPublications, Dispatcher, PublicationConfig, Publications, ReplacePublicationState,
};
use sipx_ua::presence::Compositor;

let publications = Publications::new(
    PublicationConfig::default(),
    Compositor::new(Duration::from_secs(3_600)),
    Arc::new(ReplacePublicationState),
    Arc::new(AllowPublications),
)?;
let publication_handle = publications.handle();
let dispatcher = Dispatcher::new(endpoint, incoming).with_publications(publications);

assert_eq!(publication_handle.counts().active_publications, 0);
# Ok::<(), Box<dyn std::error::Error>>(())
```

`AllowPublications` is deliberately explicit and is suitable only when an authenticated frontend
has already made the identity decision. A production endpoint implements `PublicationAuthorization`
to bind source, transport, resource and Event package to its own policy. Accepted initial,
conditional refresh, modification and removal requests receive fresh entity tags; stale, expired or
cross-resource tags fail with 412 without mutating the compositor. Body size, active resources,
publishers, queues, retries, timers and transactions are finite.

For outbound state, call `publication_handle.publish(Start { ... })` with the selected peer,
credentials and complete initial body. The returned `Publication` reports authoritative tag/expiry
changes, accepts bounded conditional `modify` and `remove` commands, and refreshes automatically at
four fifths of the granted interval. A 412 is terminal and discards the tag: the application must
start a new publication with a complete body. Dropping the handle requests removal, while dispatcher
shutdown cancels and joins owned work. Durable storage and projection from the compositor into later
presence NOTIFY documents remain application responsibilities.

## Handle application-owned dialog requests

On an established call, `Call::send_dialog_request` originates INFO and MESSAGE directly, plus an
exact private method first admitted with `Call::admit_dialog_method`. The call constructs the
Request-URI, route set, dialog identifiers and monotonic CSeq, protects stack-owned headers, enforces
a 64 KiB body ceiling, and performs one bounded digest retry from the dialog's credentials.

Incoming requests arrive as `CallEvent::ApplicationRequest`. The owned `ApplicationRequest`
preserves the validated headers and bounded body and carries an exactly-once final-response
capability. Responding, dropping the last owner, or reaching the response deadline always resolves
the server transaction. BYE, OPTIONS, re-INVITE, UPDATE, REFER, NOTIFY and other stack-owned methods
cannot be intercepted or forged through this surface.

## Runtime and feature boundaries

- `sipx-sip` and `sipx-sdp` are sans-I/O and have no async runtime.
- `sipx-testkit::call::CallHarness` asynchronously drives the real call API over socket-free SIP
  signalling; `TransactionHarness` is the seeded, nanosecond virtual-time surface whose clock
  advances only when a test asks. Its Experimental `RealtimePeer` supplies a bounded loopback
  WebSocket counterparty for the realtime bridge's deterministic protocol and failure vectors.
- `sipx-transport`, `sipx-ua`, `sipx-media`, and `sipx-call` use Tokio for I/O-facing work.
- `sipx-transport` enables UDP, TCP, DNS, TLS, WebSocket, secure WebSocket, and the Experimental
  SIP-over-QUIC mapping by default. Use `default-features = false` with an explicit feature list
  for a smaller transport build.
- `sipx-ua` enables its `runtime` feature by default. Disable defaults only when using its
  authentication and other non-runtime primitives without the transport-backed user agent.
- `sipx-media` has no default features. `opus` links the optional codec library; `dtls` links the
  optional handshake backend. Its DTLS components can key a media session through explicit policy.
- `sipx-call` exposes optional `opus` and `dtls` features. The default call codec set is G.711, and
  selecting DTLS-SRTP is always explicit.

The call crate can persist a confirmed, quiescent dialog without serializing its runtime. See
[Persist and restore a confirmed dialog](persist-a-dialog.md) for the format, fresh-driver
attachment contract, and the host responsibilities that deliberately remain outside the library.

## The API reference

Generated from the source with `cargo doc`, and published beside this site rather than only on
docs.rs so that it always matches the guides next to it. Start points:

| Crate | Start at |
|---|---|
| `sipx-sip` | [`parser`](https://codewandler.github.io/sipx/api/sipx_sip/parser/index.html) · [`transaction`](https://codewandler.github.io/sipx/api/sipx_sip/transaction/index.html) |
| `sipx-transport` | [`bind`](https://codewandler.github.io/sipx/api/sipx_transport/endpoint/fn.bind.html) · [`Handle`](https://codewandler.github.io/sipx/api/sipx_transport/endpoint/struct.Handle.html) · [`RequestPolicy`](https://codewandler.github.io/sipx/api/sipx_transport/policy/trait.RequestPolicy.html) · [`EndpointObservation`](https://codewandler.github.io/sipx/api/sipx_transport/policy/enum.EndpointObservation.html) · [`Target`](https://codewandler.github.io/sipx/api/sipx_transport/target/struct.Target.html) |
| `sipx-ua` | [`UserAgent`](https://codewandler.github.io/sipx/api/sipx_ua/agent/struct.UserAgent.html) · [`event_client`](https://codewandler.github.io/sipx/api/sipx_ua/event_client/index.html) · [`publication_client`](https://codewandler.github.io/sipx/api/sipx_ua/publication_client/index.html) |
| `sipx-sdp` | [`answer`](https://codewandler.github.io/sipx/api/sipx_sdp/answer/fn.answer.html) |
| `sipx-rtp` | [`srtp`](https://codewandler.github.io/sipx/api/sipx_rtp/srtp/index.html) · [`rtcp`](https://codewandler.github.io/sipx/api/sipx_rtp/rtcp/index.html) |
| `sipx-media` | [`MediaSession`](https://codewandler.github.io/sipx/api/sipx_media/session/struct.MediaSession.html) |
| `sipx-call` | [`dial`](https://codewandler.github.io/sipx/api/sipx_call/call/fn.dial.html) · [`answer`](https://codewandler.github.io/sipx/api/sipx_call/call/fn.answer.html) · [`Call`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html) · [`ApplicationRequest`](https://codewandler.github.io/sipx/api/sipx_call/extension/struct.ApplicationRequest.html) · [`EventSubscriptions`](https://codewandler.github.io/sipx/api/sipx_call/subscriber/struct.EventSubscriptions.html) · [`Publications`](https://codewandler.github.io/sipx/api/sipx_call/publication/struct.Publications.html) |
| `sipx-testkit` | [`CallHarness`](https://codewandler.github.io/sipx/api/sipx_testkit/call/struct.CallHarness.html) · [`TransactionHarness`](https://codewandler.github.io/sipx/api/sipx_testkit/call/struct.TransactionHarness.html) · [`RealtimePeer`](https://codewandler.github.io/sipx/api/sipx_testkit/realtime_peer/struct.RealtimePeer.html) · [`Faults`](https://codewandler.github.io/sipx/api/sipx_testkit/link/struct.Faults.html) |

The API reference is generated from the same `main` branch as this site. When using the tagged
release, consult the checked-out source documentation if an API has changed on `main`.
