---
title: Native-browser audio proof
description: What sipx proves against a native browser, how the evidence is collected, and where the result stops.
---

# Native-browser audio proof

The browser-audio profile is one deliberately narrow WebRTC-compatible path: authenticated SIP
over WSS, one audio section, host or server-reflexive ICE candidates, multiplexed RTP/RTCP on the
nominated component, DTLS-SRTP, and Opus. It is not a browser SDK or a general WebRTC engine.

The release gate runs an ordinary headless browser rather than another sipx endpoint. Its peer page
uses the browser's own `RTCPeerConnection`, WebSocket, Web Audio, and statistics interfaces. The
page contains no sipx parser, SDP generator, ICE agent, DTLS implementation, packetizer, or codec.
The sipx side is a compiled example that consumes only the public transport, call, and media APIs.

## Run the executable proof

The public-API endpoint is the `sipx-call` example `browser_audio_proof`. The harness normally
launches it with this exact shape:

```bash
cargo run -p sipx-call --example browser_audio_proof --features opus,dtls -- \
  --role browser-offerer \
  --case positive \
  --media-address "$SIPX_BROWSER_AUDIO_MEDIA_ADDRESS" \
  --cert "$SIPX_BROWSER_AUDIO_WSS_CERT" \
  --key "$SIPX_BROWSER_AUDIO_WSS_KEY" \
  --result /tmp/sipx-browser-offerer.json
```

`--role` is `browser-offerer` or `browser-answerer`. `--case` is `positive`,
`FingerprintMismatch`, `NoNominatedPair`, or `WeakerMedia`; fingerprint mismatch belongs to the
offerer role, while missing nomination and weaker media belong to the answerer role.
`--media-address` must be a non-loopback address assigned to the host so the browser can reach the
advertised ICE candidate. `--cert` and `--key` are one PEM WSS identity, and `--result` is the JSON
file the example writes after the bounded call. The process first prints a JSON listening record,
then waits for the matching native-browser peer. Running this endpoint alone is therefore not an
interoperability proof.

For the complete proof from a source checkout, first build that exact example and then run the
single harness command:

```bash
cargo build -p sipx-call --example browser_audio_proof --features opus,dtls
./tests/browser-audio/ci.sh
```

The harness generates the ephemeral WSS identity, discovers a reachable host address, starts the
matched WebDriver, runs both positive roles, one unused-RTCP-candidate compatibility call and all
three refusal cases, and validates the evidence. It exits nonzero when a compatible WebDriver,
browser, OpenSSL, or native Opus
development library is unavailable; an unavailable proof environment is not reported as a skip.
This command is repository proof infrastructure, not a browser SDK or a supported application
launcher.

## What the positive proves

The proof establishes three separate calls:

1. The browser creates the offer and sipx answers it.
2. The browser opens authenticated WSS, reports readiness with OPTIONS, and sipx places the call
   back over that exact inbound connection; the browser then creates the answer.
3. The browser creates another offer, the harness adds exactly one bounded component-two fallback
   candidate derived from its component-one host candidate, and sipx answers with mux after
   discarding that unused fallback.

In each call the browser and sipx emit separate terminal objects. The validator requires both ends
to report Opus at a 48 kHz RTP clock, connected DTLS-SRTP, a nominated component-one pair, and
nonzero media in both directions. A generated tone must be decoded as non-silent audio at each end.
The two views of every candidate pair are cross-checked in reverse: the browser's local endpoint is
sipx's remote endpoint, and vice versa. The compatibility call must show components 1 and 2 in the
sent offer, component 1 in the received answer, and component 1 as the only nominated pair. INVITE,
final response, ACK, BYE, and the BYE final response must complete in order.

Process exit is not accepted as a substitute for any of those facts.

## The three refusals

Every negative is paired with an already validated positive from the same browser role and bound to
the SHA-256 digest of that positive evidence.

- A changed SDP fingerprint keeps the browser's real certificate. ICE must nominate and reach DTLS
  certificate verification, sipx must return `FingerprintMismatch`, and no RTP may reach the
  browser. Chrome may remain in DTLS `connecting` or briefly report `connected` before observing
  the peer's typed refusal; sipx's refusal is the key-install boundary.
- For missing nomination, the browser sends a complete answer and then closes its peer connection
  immediately after ACK. ICE must have started without selecting a pair; sipx must return
  `NoNominatedPair`, with no DTLS or RTP path.
- A browser-created answer is weakened before it is applied locally or sent. sipx must return
  `WeakerMedia` before browser ICE/DTLS begins and must not try a fallback media policy.

These are layer tests, not three generic calls that happened to fail. A certificate or browser that
cannot complete the paired positive makes the negative vacuous and therefore fails the job.

## Bounds and reproducibility

The complete invocation has a five-minute bound and each role a two-minute bound. One shell owner
records every process group, caps each stdout/stderr file at 1 MiB, terminates groups on success or
failure, escalates after a finite grace period, and waits for every leader. The WSS certificate and
private key are created for one CI run; certificate chain, DNS identity, and exact public-key pin
are checked before any browser or sipx role is admitted.

The structured evidence is retained as a CI artifact. A separate adversarial self-test corrupts or
removes its facts, supplies the wrong identity, floods output, and kills a helper that forked a
grandchild. That self-test says the measuring instrument fails closed. Only the native-browser job
is compatibility evidence.

The normative contract is
[`docs/specs/browser-audio-proof.md`](https://github.com/codewandler/sipx/blob/main/docs/specs/browser-audio-proof.md).

## What it does not prove

The topology is host or server-reflexive, with one audio component. It does not cover TURN-required
networks, video, data channels, SCTP, multiple bundled media sections, simulcast, browser-facing
application APIs, or arbitrary WebRTC peers and network conditions. Those omissions remain product
boundaries, not implied follow-up claims.
