---
title: Security
description: What sipx protects today, how the CLI differs from the Rust libraries, and what remains out of reach.
---

# Security

sipx separates signalling security from media security. TLS and secure WebSocket protect a
signalling hop. SRTP protects RTP and RTCP. One does not imply the other unless the application
selects a call path that provides both.

## Capability matrix

| Capability | `sipx` CLI | Rust libraries |
|---|---|---|
| UDP and TCP signalling | Yes | Yes, through `sipx-transport` |
| TLS and secure WebSocket (WSS) | Yes, selected explicitly with `--transport tls` or `--transport wss` | Yes. `sipx-transport` supports TLS and WSS when their Cargo features are enabled |
| Certificate verification | Mandatory. Platform roots, additional PEM roots, service identity and optional mutual-TLS identity are configurable; verification cannot be disabled | Mandatory for outgoing TLS and WSS; there is no skip-verification option |
| SDES-keyed SRTP | Yes. A CLI call over TLS or WSS selects the call layer's protected-signalling path | Yes. `sipx-call` negotiates SDES-keyed SRTP when the selected signalling transport is secure |
| DTLS-SRTP | Yes, with `--media-security dtls-srtp` when the off-by-default `dtls` feature is enabled | Yes, through explicit `sipx-call::Keying::DtlsSrtp` policy with the same feature |
| ICE | Host candidates or a configured STUN server with `--ice`; disabled by default | Host and server-reflexive candidates through `sipx-call::IcePolicy`; no TURN relay |
| Signalling capture | `--capture <FILE>` writes a redacted pcapng file | `sipx-transport::CaptureConfig` enables capture; redaction is on by default, and the same redacted records can be exported to a HEP3 collector |

For a protected command-line call, select TLS or WSS and provide any private trust root explicitly;
the result reports both requested and negotiated transport. Media remains SDES-keyed SRTP until the
caller explicitly selects another policy. Explicit SDES over cleartext signalling is refused before
network I/O; strict DTLS-SRTP never falls back to SDES or plain RTP.

## TLS and WSS protect one hop

A `sips:` URI restricts sipx to TLS-capable transports. It does not promise end-to-end signalling
confidentiality: each SIP intermediary may terminate one TLS connection and open another, as
described by [RFC 3261 §26.2.2](https://www.rfc-editor.org/rfc/rfc3261#section-26.2.2). WSS applies
the same TLS policy before the WebSocket upgrade; it is not a weaker, separate certificate path.

sipx validates the certificate chain, expiry, signature, and peer identity. The identity is the
SIP domain sipx set out to reach, not an address returned by DNS. A mismatch, expired certificate,
or unknown issuer ends the connection; sipx does not retry over cleartext. Applications may choose
trust anchors and may configure a client certificate, but cannot disable verification. TLS 1.2 is
the minimum and TLS 1.3 is preferred, following [RFC 8996](https://www.rfc-editor.org/rfc/rfc8996).

## SDES and SRTP

SRTP encrypts the RTP payload and authenticates the packet. Through SDES the master key is carried
in SDP, so sipx offers it only when signalling
uses TLS or WSS. A TLS-terminating intermediary can still read that SDP key. This is a property of
SDES's threat model, not end-to-end media keying; see
[RFC 4568 §7.1](https://www.rfc-editor.org/rfc/rfc4568#section-7.1).

The implemented SRTP transform is AES counter mode with a 128-bit key and HMAC-SHA1 with an 80-bit
authentication tag. Receiving SRTP and SRTCP keep separate 64-packet replay windows: an authenticated
packet is accepted once, and authentication succeeds before either window or rollover state changes.
Packets older than the window are refused rather than accepted after waiting. Rekeying and the other
SRTP transforms are not implemented. The
[RFC compliance table](compliance.md) records these limits alongside the supported portions of
RFC 3711 and RFC 4568.

## DTLS-SRTP is an explicit call policy

DTLS-SRTP performs its handshake on the media path and carries only the certificate fingerprint
in signalling. That prevents a signalling intermediary from learning the media key merely by
terminating TLS.

`sipx-call::Keying::DtlsSrtp` selects it for dialing or answering. The call emits a fresh per-call
fingerprint, verifies the peer certificate before accepting exported keys, and runs the handshake
on the same bound port that then carries SRTP. A caller sends its ACK before beginning DTLS, so a
peer that waits for SIP confirmation cannot deadlock with the handshake.

The Cargo feature supplies the OpenSSL handshake and remains off by default. Enabling it does
**not** turn an ordinary call into a DTLS-keyed call; the default remains SDES on protected
signalling and plain RTP otherwise. Selecting DTLS in a build without the feature is a typed error,
never a cleartext fallback. Reliable early media with DTLS-SRTP is currently refused, also without
fallback. The named browser-audio profile is the one ICE + DTLS-SRTP composition: it additionally
requires authenticated WSS, RTCP multiplexing, and Opus, and refuses any missing element rather
than dropping to the ordinary media policy. The CLI exposes the same strict policy through
`--media-security` and reports the keying mode read from the established call rather than copying
the requested value.

The [native-browser proof](browser-audio-proof.md) exercises that composition in both SIP roles and
separately changes the fingerprint, prevents nomination, and supplies a weaker answer. Its coverage
is host or server-reflexive audio; TURN-required networks and broader WebRTC behavior remain outside
it.

## Captures remain sensitive

The CLI's `--capture` option records SIP signalling, not media. It redacts digest responses,
opaque authorization tokens, SDES keys, SDP `k=` values, push tokens, and instance identifiers.
TLS and WSS traffic is captured after decryption so that the signalling remains diagnosable.

Redaction does not anonymize a call. Names, SIP addresses, network addresses, timing, routes, and
other call metadata remain. Treat every capture as sensitive data: store it with restricted
permissions, share it only with intended recipients, and delete it when the investigation ends.
The library can disable capture redaction for tightly controlled diagnostics; the CLI deliberately
does not expose that option. HEP3 network export cannot disable redaction: an endpoint combining
the collector with the lab-only opt-out is refused before binding. HEP transport is best-effort UDP,
so use a trusted network path and collector access controls; redacted signalling still identifies
participants and addresses.

For operational symptoms and capture commands, see [Troubleshooting](../guides/troubleshooting.md).
Library applications can configure the collector and application-owned RTCP quality hook through
[Export signalling and call quality](../guides/export-observability.md).
