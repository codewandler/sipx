---
title: Integrate with an existing SIP system
description: Add sipx endpoints and call applications gradually without confusing endpoint, proxy, registrar, and application roles.
---

# Integrate with an existing SIP system

You rarely need to replace a working SIP service to adopt sipx. Start by identifying the role you
need, then add sipx at an endpoint boundary where its behavior can be tested independently.

## Map the roles first

| SIP role | What it does | sipx status |
|---|---|---|
| User agent client | Originates requests and calls | Shipped in the CLI and Rust libraries |
| User agent server | Receives requests and answers calls | Shipped in the CLI and Rust libraries |
| Registration client | Publishes an endpoint's Contact as a lease | Shipped in `sipx register` and `sipx-ua` |
| Proxy | Routes or forks other endpoints' requests and maintains a route set | Not a role provided by this repository |
| Registrar and location service | Accepts and stores registrations for other endpoints | Not a role provided by this repository |
| Call application | Plays, records, gathers digits, transfers, and owns call behavior | Shipped as Rust APIs; the language-neutral application contract is Experimental |

These roles are distinct in [RFC 3261](https://www.rfc-editor.org/rfc/rfc3261). Registering a sipx
endpoint with an existing registrar does not make sipx a registrar, and accepting an incoming call
does not make it a proxy.

## Choose a narrow first workload

Good first integrations are endpoints with explicit inputs and outputs:

- a health probe that places a call and checks the result;
- an announcement endpoint that answers, plays a WAV file, and records the caller;
- a notification dialler that reports JSON and branches on the exit code;
- an endpoint that subscribes to an existing registrar's registration event package and reports the
  current contacts without taking ownership of registration storage;
- a Rust service that registers one endpoint and owns its calls.

Keep routing, registration storage, queues, and other network-wide policy in the system that
already owns those roles. A direct endpoint workload establishes signalling, authentication,
audio, and failure behavior without making a cutover depend on unfinished application surfaces.

## Add a CLI endpoint

For a direct test, start an answerer on an address your existing system can reach. Replace the
documentation address below with the endpoint's reachable interface address:

```bash
sipx answer --local 192.0.2.20:5070 --play greeting.wav --record caller.wav --wait 60
```

Route one test destination to that address and port. For an outbound probe, call a concrete target
and capture a machine-readable result:

```bash
sipx dial sip:probe@192.0.2.10:5060 --play probe.wav --timeout 15 --json
```

The CLI consumes and produces WAV files by default. Builds with the optional `device-audio` feature
can open an exact microphone or speaker identifier. It selects UDP, TCP, TLS, WS, or WSS
signalling; secure signalling uses mandatory certificate verification, and calls expose explicit
plain-RTP, SDES-SRTP, or optional DTLS-SRTP policy. See the
[CLI reference](../reference/cli.md) and [Security](../reference/security.md) before treating the
probe as a production security test.

## Register an endpoint

When the existing system requires registration, treat it as a renewable lease:

```bash
SIPX_PASSWORD='secret' sipx register sip:alice@example.net \
  --target 192.0.2.10:5060 --keep-alive
```

Use RFC 5626 Outbound when calls must return down a client-opened flow:

```bash
SIPX_PASSWORD='secret' sipx register sip:alice@example.net \
  --target 192.0.2.10:5060 --outbound --instance urn:uuid:YOUR-STABLE-ID --keep-alive
```

Persist the instance URN across restarts. Registration and call answering are separate CLI
processes, so a long-lived production endpoint is usually clearer as one Rust application using
`sipx-ua` and `sipx-call` on the same transport endpoint.

## Move the durable endpoint into Rust

Use the Rust libraries when the integration needs one process to register, answer, place calls,
service in-dialog requests, or select protected transports. The public guides show the smallest
complete paths:

- [Choose the library crates](as-a-library.md)
- [Place a call](place-a-call.md)
- [Answer a call](answer-a-call.md)
- [Register](register.md)

Set the bound and advertised addresses explicitly. `sipx dial` and `sipx answer` take
`--local` and `--advertise` separately; the terminal JSON reports `media_bound` and
`media_advertised` so a deployment can verify the choice. Behind NAT, sipx can use symmetric RTP or
gather host and STUN-derived server-reflexive ICE candidates. TURN and relayed candidates are not
available, so some NAT pairs still have no working media path. Validate from the network where the
endpoint will actually run, not only on loopback.

## Name a destination instead of addressing one

Your application does not have to turn names into addresses. `sipx_transport::destination::Resolver`
performs the same RFC 3263 lookup the CLI performs — NAPTR, then SRV, then A and AAAA — and returns
the ordered `Target` candidates to try. Build it with `Resolver::within(budget)`, which states the
deadline the lookup has to fit inside; the lookup is bounded at two seconds per question and eight
seconds overall, or less when your budget is shorter.

Two guarantees come with it, and both are easy to lose when an application resolves names on its
own. A `sips:` URI never yields a cleartext candidate, so a secure destination cannot silently
downgrade. And every secure candidate keeps the host you named as its TLS or WSS verification
identity, so an address chosen by DNS never chooses which certificate is acceptable.

`sipx_ua::Config::resolved` does the same for a registrar you can only name, so a long-lived
endpoint does not hold an address its DNS record has already replaced. Resolution failure,
resolution timeout, and connection failure stay distinct: the first two are reported before
anything is dialled, and `Error::kind` separates a zone with no answer from a deadline.

`with_service_route` writes the preloaded `Route` headers but does not resolve them. Resolve the
outermost proxy with the same resolver, pass the first candidate as the `Target` to `dial`, and
keep the rest as the order to fall back through; the called party remains in the Request-URI.

For a long-lived endpoint, keep operational policy at the transport boundary. A host can rotate the
TLS identity used by new handshakes, attach one bounded message/connection observer, atomically
replace the admitted source-prefix set, and install an immutable pre-transaction request policy.
Those seams do not replace routing or authorization: protected SIP fields cannot be rewritten, and
established connections keep the identity and source-admission generation that accepted them. See
[Use sipx as a library](as-a-library.md#operate-a-live-transport-endpoint) for the exact API boundary.

The same Rust process can attach inbound and outbound event services, registration discovery, and
conditional presence publication to its dispatcher. Established calls can also surface and answer
application-owned INFO or MESSAGE requests without exposing the stack-owned BYE, negotiation, or
transfer paths. The corresponding ownership and bounds are in
[the library guide](as-a-library.md#handle-application-owned-dialog-requests).

## Know the application boundary

Rust applications can already answer, play, record, send or collect DTMF, and transfer. Do not plan
a proxy or registrar replacement around those endpoint APIs. Public early and confirmed coupling
owns two dialogs and can attach the bounded media bridge. It does not yet provide the truly
off-media relay role, and routing policy remains application work.

`sipx-host` can bind a SIP listener and serve calls to document-mode webhooks, authenticated
full-duplex sessions, or a configured realtime audio bridge; a granted session can also originate
a call. Those Rust host surfaces are Supported under the pre-1.0 policy. The language-neutral wire
contract remains Experimental, and there is no embedded runtime or TypeScript SDK. Its precise status is on the
[application host overview](../sdk/overview.md).

## Validate before expanding

For each new endpoint, verify:

1. success and refusal outcomes are distinguishable;
2. authentication fails closed with a wrong credential;
3. Contact, Via, and SDP addresses are reachable from the real network;
4. audio flows in both directions for the full call;
5. BYE and timeout paths release signalling and media resources;
6. TLS certificate failures are refused when the Rust application uses TLS or WSS.

Expand destinations only after those checks pass. [Troubleshooting](troubleshooting.md) maps the
common symptoms to the address, NAT, media, authentication, and capture checks that settle them.
